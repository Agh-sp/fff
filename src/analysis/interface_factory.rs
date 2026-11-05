//! Resolve only small, recognized x64 interface factories. Never execute target
//! code or assume arbitrary instruction bytes contain a RIP displacement.
use std::collections::HashSet;

use anyhow::{Context, Result, bail, ensure};

const MAX_STEPS: usize = 16;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FactoryResult {
    /// Address relative to the module containing the interface registration.
    Static(u64),
    /// This factory cannot be represented by the output's module-relative value.
    Omitted(String),
}

fn module_rva(base: u64, size: u64, address: u64) -> Option<u64> {
    address.checked_sub(base).filter(|&rva| rva < size)
}

fn relative_target(ip: u64, len: u64, displacement: i64) -> Result<u64> {
    ip.checked_add(len)
        .and_then(|end| end.checked_add_signed(displacement))
        .context("interface factory relative address overflow")
}

fn read_bytes<const N: usize>(
    read: &mut impl FnMut(u64, &mut [u8]) -> Result<()>,
    base: u64,
    size: u64,
    address: u64,
) -> Result<[u8; N]> {
    let rva = module_rva(base, size, address).context("factory read outside module")?;
    ensure!(
        N as u64 <= size - rva,
        "factory read crosses module boundary"
    );
    let mut bytes = [0; N];
    read(address, &mut bytes)
        .with_context(|| format!("failed to read factory bytes at {address:#X}"))?;
    Ok(bytes)
}

/// Supports `lea rax, [rip+disp32]; ret`, `mov rax, [rip+disp32]; ret`,
/// null-return stubs, and bounded direct/indirect jump thunks. Other factory
/// implementations are explicitly omitted rather than decoded speculatively.
pub(super) fn resolve_factory(
    base: u64,
    size: u64,
    entry: u64,
    mut read: impl FnMut(u64, &mut [u8]) -> Result<()>,
) -> Result<FactoryResult> {
    ensure!(
        size > 0 && base.checked_add(size).is_some(),
        "invalid interface module range"
    );
    let mut ip = entry;
    let mut seen = HashSet::new();
    for _ in 0..MAX_STEPS {
        if module_rva(base, size, ip).is_none() {
            return Ok(FactoryResult::Omitted(format!(
                "factory code at {ip:#X} is outside the registering module"
            )));
        }
        ensure!(seen.insert(ip), "cycle in interface factory at {ip:#X}");
        let [opcode] = read_bytes::<1>(&mut read, base, size, ip)?;
        match opcode {
            0xE9 => {
                let bytes = read_bytes::<5>(&mut read, base, size, ip)?;
                let displacement = i32::from_le_bytes(bytes[1..5].try_into()?) as i64;
                ip = relative_target(ip, 5, displacement)?;
                continue;
            }
            0xEB => {
                let bytes = read_bytes::<2>(&mut read, base, size, ip)?;
                ip = relative_target(ip, 2, bytes[1] as i8 as i64)?;
                continue;
            }
            0xFF => {
                let bytes = read_bytes::<2>(&mut read, base, size, ip)?;
                if bytes[1] == 0x25 {
                    let bytes = read_bytes::<6>(&mut read, base, size, ip)?;
                    let displacement = i32::from_le_bytes(bytes[2..6].try_into()?) as i64;
                    let slot = relative_target(ip, 6, displacement)?;
                    ip = u64::from_le_bytes(read_bytes::<8>(&mut read, base, size, slot)?);
                    continue;
                }
            }
            0x90 => {
                ip = relative_target(ip, 1, 0)?;
                continue;
            }
            0xF3 => {
                if read_bytes::<4>(&mut read, base, size, ip)? == [0xF3, 0x0F, 0x1E, 0xFA] {
                    ip = relative_target(ip, 4, 0)?; // ENDBR64
                    continue;
                }
            }
            0x31 | 0x33 => {
                let bytes = read_bytes::<3>(&mut read, base, size, ip)?;
                if bytes[1..] == [0xC0, 0xC3] {
                    return Ok(FactoryResult::Omitted("factory returns null".into()));
                }
            }
            0x48 => {
                let header = read_bytes::<3>(&mut read, base, size, ip)?;
                if matches!(header[1], 0x31 | 0x33) && header[2] == 0xC0 {
                    if read_bytes::<4>(&mut read, base, size, ip)?[3] == 0xC3 {
                        return Ok(FactoryResult::Omitted("factory returns null".into()));
                    }
                } else if matches!(header[1], 0x8D | 0x8B) && header[2] == 0x05 {
                    let bytes = read_bytes::<8>(&mut read, base, size, ip)?;
                    if bytes[7] == 0xC3 {
                        let displacement = i32::from_le_bytes(bytes[3..7].try_into()?) as i64;
                        let target = relative_target(ip, 7, displacement)?;
                        // MOV returns the pointer stored in the slot; LEA returns
                        // the slot address itself. They are not interchangeable.
                        let instance = if header[1] == 0x8B {
                            u64::from_le_bytes(read_bytes::<8>(&mut read, base, size, target)?)
                        } else {
                            target
                        };
                        if instance == 0 {
                            return Ok(FactoryResult::Omitted("factory returns null".into()));
                        }
                        return Ok(match module_rva(base, size, instance) {
                            Some(rva) => FactoryResult::Static(rva),
                            None => FactoryResult::Omitted(format!(
                                "instance {instance:#X} has no RVA in the registering module"
                            )),
                        });
                    }
                }
            }
            _ => {}
        }
        return Ok(FactoryResult::Omitted(format!(
            "unsupported factory instructions at {ip:#X} (first opcode {opcode:#04X})"
        )));
    }
    bail!("interface factory exceeds {MAX_STEPS} decoding steps")
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    const BASE: u64 = 0x1000;

    fn resolve(image: &[u8], offset: usize) -> Result<FactoryResult> {
        resolve_factory(
            BASE,
            image.len() as u64,
            BASE + offset as u64,
            |addr, out| {
                let start = usize::try_from(addr - BASE)?;
                out.copy_from_slice(
                    image
                        .get(start..start + out.len())
                        .context("unmapped bytes")?,
                );
                Ok(())
            },
        )
    }

    fn lea(image: &mut [u8], at: usize, target: usize) {
        image[at..at + 3].copy_from_slice(&[0x48, 0x8D, 0x05]);
        image[at + 3..at + 7].copy_from_slice(&((target as i32) - (at as i32 + 7)).to_le_bytes());
        image[at + 7] = 0xC3;
    }

    #[test]
    fn resolves_positive_and_negative_rip_displacements() {
        let mut image = [0; 128];
        lea(&mut image, 0, 64);
        lea(&mut image, 64, 16);
        assert_eq!(resolve(&image, 0).unwrap(), FactoryResult::Static(64));
        assert_eq!(resolve(&image, 64).unwrap(), FactoryResult::Static(16));
    }

    #[test]
    fn null_stubs_do_not_turn_padding_into_an_address() {
        for code in [
            vec![0x33, 0xC0, 0xC3],
            vec![0x31, 0xC0, 0xC3],
            vec![0x48, 0x33, 0xC0, 0xC3],
        ] {
            assert_eq!(
                resolve(&code, 0).unwrap(),
                FactoryResult::Omitted("factory returns null".into())
            );
        }
    }

    #[test]
    fn prologue_and_nonreturning_lea_are_not_guessed() {
        assert!(matches!(
            resolve(&[0x48, 0x83, 0xEC, 0x28], 0).unwrap(),
            FactoryResult::Omitted(_)
        ));
        let mut image = [0; 64];
        lea(&mut image, 0, 32);
        image[7] = 0x90;
        assert!(matches!(
            resolve(&image, 0).unwrap(),
            FactoryResult::Omitted(_)
        ));
    }

    #[test]
    fn dereferences_mov_instead_of_publishing_pointer_slot() {
        let mut image = [0; 128];
        lea(&mut image, 0, 32);
        image[1] = 0x8B;
        image[32..40].copy_from_slice(&(BASE + 96).to_le_bytes());
        assert_eq!(resolve(&image, 0).unwrap(), FactoryResult::Static(96));
        image[32..40].fill(0);
        assert_eq!(
            resolve(&image, 0).unwrap(),
            FactoryResult::Omitted("factory returns null".into())
        );
    }

    #[test]
    fn does_not_publish_external_instances_as_rvas() {
        let mut image = [0; 64];
        for target in [64, 128] {
            lea(&mut image, 0, target);
            assert!(matches!(
                resolve(&image, 0).unwrap(),
                FactoryResult::Omitted(_)
            ));
        }
        image[3..7].copy_from_slice(&(-8_i32).to_le_bytes()); // base - 1
        assert!(matches!(
            resolve(&image, 0).unwrap(),
            FactoryResult::Omitted(_)
        ));
    }

    #[test]
    fn follows_direct_and_indirect_thunks() {
        let mut image = [0; 128];
        image[..5].copy_from_slice(&[0xE9, 11, 0, 0, 0]); // -> 16
        image[16..18].copy_from_slice(&[0xEB, 14]); // -> 32
        image[32..38].copy_from_slice(&[0xFF, 0x25, 10, 0, 0, 0]); // slot 48
        image[48..56].copy_from_slice(&(BASE + 64).to_le_bytes());
        lea(&mut image, 64, 96);
        assert_eq!(resolve(&image, 0).unwrap(), FactoryResult::Static(96));
    }

    #[test]
    fn supports_endbr64_and_nop_prefixes() {
        let mut image = [0; 64];
        image[..5].copy_from_slice(&[0xF3, 0x0F, 0x1E, 0xFA, 0x90]);
        lea(&mut image, 5, 32);
        assert_eq!(resolve(&image, 0).unwrap(), FactoryResult::Static(32));
    }

    #[test]
    fn rejects_cycles_and_excessive_thunks() {
        assert!(
            resolve(&[0xEB, 0xFE], 0)
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );
        assert!(
            resolve(&[0x90; 32], 0)
                .unwrap_err()
                .to_string()
                .contains("steps")
        );
    }

    #[test]
    fn read_errors_remain_fatal() {
        let err = resolve_factory(BASE, 64, BASE, |_, _| Err(anyhow!("memory: partial data")))
            .unwrap_err();
        assert!(format!("{err:#}").contains("memory: partial data"));
        assert!(resolve(&[0x48, 0x8D, 0x05], 0).is_err());
    }

    #[test]
    fn unsupported_entry_does_not_discard_neighboring_static_entries() {
        let mut image = [0; 128];
        lea(&mut image, 0, 96);
        image[16..19].copy_from_slice(&[0x33, 0xC0, 0xC3]);
        image[32..36].copy_from_slice(&[0x48, 0x83, 0xEC, 0x28]);
        lea(&mut image, 48, 112);
        let resolved: Vec<_> = [0, 16, 32, 48]
            .into_iter()
            .map(|offset| resolve(&image, offset).unwrap())
            .collect();
        assert_eq!(resolved[0], FactoryResult::Static(96));
        assert!(matches!(resolved[1], FactoryResult::Omitted(_)));
        assert!(matches!(resolved[2], FactoryResult::Omitted(_)));
        assert_eq!(resolved[3], FactoryResult::Static(112));
    }

    #[test]
    fn checks_address_arithmetic() {
        assert!(relative_target(0, 1, -2).is_err());
        assert!(relative_target(u64::MAX, 1, 0).is_err());
        assert!(
            resolve_factory(u64::MAX, 2, u64::MAX, |_, _| {
                panic!("invalid range must not be read")
            })
            .is_err()
        );
    }
}
