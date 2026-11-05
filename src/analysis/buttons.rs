use std::collections::BTreeMap;

use anyhow::{Result, bail, ensure};

use log::debug;

use memflow::prelude::v1::*;

use pelite::pattern;
use pelite::pe64::{Pe, PeView};

use crate::memory::validation::TraversalGuard;
use crate::source2::KeyButton;

pub type ButtonMap = BTreeMap<String, umem>;

pub fn buttons<P: Process + MemoryView>(process: &mut P) -> Result<ButtonMap> {
    let module = process.module_by_name("client.dll")?;

    let buf = process
        .read_raw(module.base, module.size as _)
        .data_part()?;

    let view = PeView::from_bytes(&buf)?;

    let mut save = [0; 2];

    if !view
        .scanner()
        .finds_code(pattern!("488b15${'} 4885d2 74? 488b02 4885c0"), &mut save)
    {
        bail!("outdated button list pattern");
    }

    let list_head = process.read_addr64(module.base + save[1]).data()?;

    read_buttons(process, &module, list_head)
}

fn read_buttons(
    mem: &mut impl MemoryView,
    module: &ModuleInfo,
    list_head: Address,
) -> Result<ButtonMap> {
    let mut result = ButtonMap::new();

    let mut button_ptr = Pointer64::<KeyButton>::from(list_head);

    let mut guard = TraversalGuard::default();
    while !button_ptr.is_null() {
        guard.visit(button_ptr.address().to_umem() as u64)?;
        let button = mem.read_ptr(button_ptr).data()?;
        let name = mem.read_utf8_lossy(button.name.address(), 32).data_part()?;

        let state_addr = button_ptr.address() + offset_of!(KeyButton.state);

        ensure!(!name.is_empty(), "empty buttons name");
        let state_rva = state_addr
            .to_umem()
            .checked_sub(module.base.to_umem())
            .ok_or_else(|| anyhow::anyhow!("buttons address precedes module base"))?;
        ensure!(
            state_rva < module.size as umem,
            "buttons address outside module"
        );
        debug!(
            "found \"{}\" at {:#X} ({} + {:#X})",
            name,
            state_addr.to_umem(),
            module.name,
            state_rva
        );

        result.insert(name, state_rva);

        button_ptr = button.next;
    }

    ensure!(!result.is_empty(), "no buttons found");
    Ok(result)
}
