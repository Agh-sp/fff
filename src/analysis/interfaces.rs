use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};

use log::debug;

use memflow::prelude::v1::*;

use pelite::pe64::exports::Export;
use pelite::pe64::{Pe, PeView};

use crate::memory::address;
use crate::memory::validation::TraversalGuard;
use crate::source2::InterfaceReg;

pub type InterfaceMap = BTreeMap<String, BTreeMap<String, umem>>;

pub fn interfaces<P: Process + MemoryView>(process: &mut P) -> Result<InterfaceMap> {
    let mut result = InterfaceMap::new();
    for module in process.module_list()? {
        let buf = process
            .read_raw(module.base, module.size as _)
            .data_part()
            .with_context(|| format!("failed to read module {}", module.name))?;
        let view = PeView::from_bytes(&buf)?;

        // Modules without this export are normal, not failed interface reads.
        let ci_export = view
            .exports()
            .and_then(|exports| exports.by())
            .and_then(|exports| exports.name("CreateInterface"));
        let Ok(Export::Symbol(symbol)) = ci_export else {
            continue;
        };

        let list_ptr = address::resolve_rip(process, module.base + symbol)?;
        let list_head = process.read_addr64(list_ptr).data()?;
        let interfaces = read_interfaces(process, &module, list_head)
            .with_context(|| format!("failed to read interfaces in {}", module.name))?;
        if !interfaces.is_empty() {
            result.insert(module.name.to_string(), interfaces);
        }
    }
    ensure!(!result.is_empty(), "no interfaces found");
    Ok(result)
}

fn read_interfaces(
    mem: &mut impl MemoryView,
    module: &ModuleInfo,
    list_head: Address,
) -> Result<BTreeMap<String, umem>> {
    let mut result = BTreeMap::new();

    let mut reg_ptr = Pointer64::<InterfaceReg>::from(list_head);

    let mut guard = TraversalGuard::default();
    while !reg_ptr.is_null() {
        guard.visit(reg_ptr.address().to_umem() as u64)?;
        let reg = mem.read_ptr(reg_ptr).data()?;
        let name = mem.read_utf8_lossy(reg.name.address(), 128).data_part()?;

        let instance_addr = address::resolve_rip(mem, reg.create_fn.address())?;

        ensure!(!name.is_empty(), "empty interfaces name");
        let instance_rva = instance_addr
            .to_umem()
            .checked_sub(module.base.to_umem())
            .ok_or_else(|| anyhow::anyhow!("interfaces address precedes module base"))?;
        ensure!(
            instance_rva < module.size as umem,
            "interfaces address outside module"
        );
        debug!(
            "found \"{}\" at {:#X} ({} + {:#X})",
            name,
            instance_addr.to_umem(),
            module.name,
            instance_rva
        );

        result.insert(name, instance_rva);

        reg_ptr = reg.next;
    }

    Ok(result)
}
