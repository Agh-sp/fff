use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};

use log::debug;

use memflow::prelude::v1::*;

use pelite::pe64::exports::Export;
use pelite::pe64::{Pe, PeView};

use crate::memory::address;
use crate::memory::validation::TraversalGuard;
use crate::source2::InterfaceReg;

use super::AnalysisReport;
use super::interface_factory::{FactoryResult, resolve_factory};
use super::interface_policy::module_result;

pub type InterfaceMap = BTreeMap<String, BTreeMap<String, umem>>;

pub fn interfaces<P: Process + MemoryView>(
    process: &mut P,
    report: &mut AnalysisReport,
) -> Result<InterfaceMap> {
    let mut result = InterfaceMap::new();
    for module in process.module_list()? {
        // Keep the entire module read inside the error boundary: unsupported
        // exports may fail while resolving the list, not just while walking it.
        let module_name = module.name.to_string();
        let value = read_module_interfaces(process, &module, report);
        let error = value.as_ref().err().map(|error| format!("{error:#}"));
        let value = module_result(&module_name, value);
        if matches!(&value, Ok(None)) {
            report.note(
                "interfaces.optional_module",
                &module_name,
                error.unwrap_or_default(),
            );
        }
        let interfaces = report
            .recover("interfaces.module", &module_name, value)?
            .flatten();
        if let Some(interfaces) = interfaces.filter(|interfaces| !interfaces.is_empty()) {
            result.insert(module_name, interfaces);
        }
    }
    ensure!(!result.is_empty(), "no interfaces found");
    Ok(result)
}

fn read_module_interfaces(
    mem: &mut impl MemoryView,
    module: &ModuleInfo,
    report: &mut AnalysisReport,
) -> Result<BTreeMap<String, umem>> {
    let buf = mem
        .read_raw(module.base, module.size as _)
        .data_part()
        .context("failed to read module image")?;
    let view = PeView::from_bytes(&buf).context("invalid PE image")?;

    // Modules without this export are normal, not failed interface reads.
    let ci_export = view
        .exports()
        .and_then(|exports| exports.by())
        .and_then(|exports| exports.name("CreateInterface"));
    let Ok(Export::Symbol(symbol)) = ci_export else {
        return Ok(BTreeMap::new());
    };

    let export_addr = module.base + symbol;
    let list_ptr = address::resolve_rip(mem, export_addr)
        .with_context(|| format!("failed to resolve CreateInterface at {export_addr:?}"))?;
    let list_head = mem
        .read_addr64(list_ptr)
        .data()
        .with_context(|| format!("failed to read interface list head at {list_ptr:?}"))?;
    read_interfaces(mem, module, list_head, report)
}

fn read_interfaces(
    mem: &mut impl MemoryView,
    module: &ModuleInfo,
    list_head: Address,
    report: &mut AnalysisReport,
) -> Result<BTreeMap<String, umem>> {
    let mut result = BTreeMap::new();

    let mut reg_ptr = Pointer64::<InterfaceReg>::from(list_head);

    let mut guard = TraversalGuard::default();
    while !reg_ptr.is_null() {
        guard.visit(reg_ptr.address().to_umem() as u64)?;
        let reg = mem.read_ptr(reg_ptr).data().with_context(|| {
            format!(
                "failed to read interface registration at {:?}",
                reg_ptr.address()
            )
        })?;
        let name = mem
            .read_utf8_lossy(reg.name.address(), 128)
            .data_part()
            .with_context(|| {
                format!("failed to read interface name at {:?}", reg.name.address())
            })?;

        ensure!(!name.is_empty(), "empty interfaces name");
        let factory = reg.create_fn.address();
        let resolved = resolve_factory(
            module.base.to_umem() as u64,
            module.size as u64,
            factory.to_umem() as u64,
            |address, bytes| {
                mem.read_raw_into(address.into(), bytes).data()?;
                Ok(())
            },
        )
        .with_context(|| {
            format!(
                "failed to resolve interface {name:?} in {} at factory {factory:?}",
                module.name
            )
        })?;

        match resolved {
            FactoryResult::Static(rva) => {
                debug!(
                    "found {:?} at {:#X} ({} + {:#X})",
                    name,
                    module.base.to_umem() + rva as umem,
                    module.name,
                    rva
                );
                result.insert(name, rva as umem);
            }
            FactoryResult::Omitted(reason) => {
                // Keep valid entries and continue via the registration's next
                // pointer, not through arbitrary factory machine code.
                report.note(
                    "interfaces.factory",
                    &format!("{}:{name}", module.name),
                    format!("omitted factory {factory:?}: {reason}"),
                );
            }
        }

        reg_ptr = reg.next;
    }

    Ok(result)
}
