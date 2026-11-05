pub use buttons::*;
pub use interfaces::*;
pub use offsets::*;
pub use report::*;
pub use schemas::*;

use anyhow::{Result, ensure};

use log::info;

use memflow::prelude::v1::*;

mod buttons;
mod interface_factory;
mod interface_policy;
mod interfaces;
mod offsets;
mod report;
mod schema_parent;
mod schemas;

#[derive(Debug)]
pub struct AnalysisResult {
    pub buttons: ButtonMap,
    pub interfaces: InterfaceMap,
    pub offsets: OffsetMap,
    pub schemas: SchemaMap,
    pub report: AnalysisReport,
}

pub fn analyze_all<P: Process + MemoryView>(
    process: &mut P,
    allow_partial: bool,
) -> Result<AnalysisResult> {
    let mut report = AnalysisReport::new(allow_partial);
    let value = buttons(process);
    let buttons = report
        .recover("buttons", "all modules", value)?
        .unwrap_or_default();

    info!("found {} buttons", buttons.len());

    let value = interfaces(process, &mut report);
    let interfaces = report
        .recover("interfaces", "all modules", value)?
        .unwrap_or_default();

    info!(
        "found {} interfaces across {} modules",
        interfaces
            .iter()
            .map(|(_, ifaces)| ifaces.len())
            .sum::<usize>(),
        interfaces.len()
    );

    let value = offsets(process, &mut report);
    let offsets = report
        .recover("offsets", "all modules", value)?
        .unwrap_or_default();

    info!(
        "found {} offsets across {} modules",
        offsets
            .iter()
            .map(|(_, offsets)| offsets.len())
            .sum::<usize>(),
        offsets.len()
    );

    let value = schemas(process, &mut report);
    let schemas = report
        .recover("schemas", "all modules", value)?
        .unwrap_or_default();

    let (class_count, enum_count) =
        schemas
            .values()
            .fold((0, 0), |(classes, enums), (class_vec, enum_vec)| {
                (classes + class_vec.len(), enums + enum_vec.len())
            });

    info!(
        "found {} classes and {} enums across {} modules",
        class_count,
        enum_count,
        schemas.len()
    );

    ensure!(
        !buttons.is_empty()
            || interfaces.values().any(|items| !items.is_empty())
            || offsets.values().any(|items| !items.is_empty())
            || schemas
                .values()
                .any(|(classes, enums)| !classes.is_empty() || !enums.is_empty()),
        "no usable data extracted; no output will be published ({} recorded issues)",
        report.issues.len()
    );
    Ok(AnalysisResult {
        buttons,
        interfaces,
        offsets,
        schemas,
        report,
    })
}
