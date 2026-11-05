use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};

use log::debug;

use memflow::prelude::v1::*;

use pelite::pattern;
use pelite::pe64::{Pe, PeView};

use serde::{Deserialize, Serialize};

use crate::memory::validation::{bounded_name, checked_count};
use crate::source2::*;

use super::AnalysisReport;
use super::schema_parent::{read_parent_name, resolve_parent};

pub type SchemaMap = BTreeMap<String, (Vec<Class>, Vec<Enum>)>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum ClassMetadata {
    Unknown { name: String },
    NetworkChangeCallback { name: String },
    NetworkVarNames { name: String, type_name: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Class {
    pub name: String,
    pub module_name: String,
    pub parent_name: Option<String>,
    /// Present only when the class declares a parent but its metadata is unresolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_error: Option<String>,
    pub metadata: Vec<ClassMetadata>,
    pub fields: Vec<ClassField>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClassField {
    pub name: String,
    pub type_name: String,
    pub offset: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Enum {
    pub name: String,
    pub alignment: u8,
    pub size: u16,
    pub members: Vec<EnumMember>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnumMember {
    pub name: String,
    pub value: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TypeScope {
    pub module_name: String,
    pub classes: Vec<Class>,
    pub enums: Vec<Enum>,
}

pub fn schemas<P: Process + MemoryView>(
    process: &mut P,
    report: &mut AnalysisReport,
) -> Result<SchemaMap> {
    let schema_system = read_schema_system(process)?;
    let type_scopes = read_type_scopes(process, &schema_system, report)?;

    Ok(type_scopes
        .into_iter()
        .map(|type_scope| {
            (
                type_scope.module_name,
                (type_scope.classes, type_scope.enums),
            )
        })
        .collect())
}

fn read_class_binding(
    mem: &mut impl MemoryView,
    binding_ptr: Pointer64<SchemaClassBinding>,
) -> Result<Class> {
    let binding = mem.read_ptr(binding_ptr).data()?;

    let module_name = mem
        .read_utf8_lossy(binding.module_name.address(), 128)
        .data_part()
        .map(|m| format!("{}.dll", m))?;

    let name = mem
        .read_utf8_lossy(binding.name.address(), 128)
        .data_part()?;

    if name.is_empty() {
        bail!("invalid class name");
    }

    let parent = resolve_parent(binding.has_base_class, || {
        ensure!(
            !binding.base_classes.is_null(),
            "class declares a parent but base_classes is null"
        );
        let base_class = mem.read_ptr(binding.base_classes).data().with_context(|| {
            format!(
                "failed to read base-class record at {:?}",
                binding.base_classes.address()
            )
        })?;
        ensure!(
            !base_class.class.is_null(),
            "base-class record contains a null class pointer"
        );
        let parent_class = mem.read_ptr(base_class.class).data().with_context(|| {
            format!(
                "failed to read parent binding at {:?}",
                base_class.class.address()
            )
        })?;
        ensure!(
            !parent_class.name.is_null(),
            "parent binding contains a null name pointer"
        );
        let name_address = parent_class.name.address().to_umem();
        read_parent_name(|index| {
            let address = name_address
                .checked_add(index as umem)
                .context("parent name address overflow")?;
            Ok(mem
                .read::<u8>(address.into())
                .data()
                .with_context(|| format!("failed to read parent name byte at {address:#X}"))?)
        })
    });

    let fields = read_class_binding_fields(mem, &binding)
        .with_context(|| format!("failed to read fields for {module_name}:{name}"))?;
    let metadata = read_class_binding_metadata(mem, &binding)
        .with_context(|| format!("failed to read metadata for {module_name}:{name}"))?;

    Ok(Class {
        name,
        module_name,
        parent_name: parent.name,
        parent_error: parent.error,
        metadata,
        fields,
    })
}

fn read_class_binding_fields(
    mem: &mut impl MemoryView,
    binding: &SchemaClassBinding,
) -> Result<Vec<ClassField>> {
    let count = checked_count(i32::from(binding.field_count))?;
    ensure!(
        count == 0 || !binding.fields.is_null(),
        "null class fields with nonzero count"
    );

    (0..count).try_fold(Vec::new(), |mut acc, i| {
        let field = mem.read_ptr(binding.fields.at(i as _)).data()?;

        ensure!(!field.r#type.is_null(), "null field type");
        ensure!(field.offset >= 0, "negative class field offset");

        let name = mem.read_utf8_lossy(field.name.address(), 128).data_part()?;
        let r#type = mem.read_ptr(field.r#type).data()?;

        let type_name = mem
            .read_utf8_lossy(r#type.name.address(), 128)
            .data_part()?
            .replace(" ", "");

        acc.push(ClassField {
            name,
            type_name,
            offset: field.offset,
        });

        Ok(acc)
    })
}

fn read_class_binding_metadata(
    mem: &mut impl MemoryView,
    binding: &SchemaClassBinding,
) -> Result<Vec<ClassMetadata>> {
    let count = checked_count(i32::from(binding.static_metadata_count))?;
    ensure!(
        count == 0 || !binding.static_metadata.is_null(),
        "null metadata with nonzero count"
    );

    (0..count).try_fold(Vec::new(), |mut acc, i| {
        let metadata = mem.read_ptr(binding.static_metadata.at(i as _)).data()?;

        let name = mem
            .read_utf8_lossy(metadata.name.address(), 128)
            .data_part()?;

        let metadata = match name.as_str() {
            "MNetworkChangeCallback" => unsafe {
                ensure!(
                    !metadata.network_value.is_null(),
                    "null network callback metadata"
                );
                let network_value = mem.read_ptr(metadata.network_value).data()?;
                let name = mem
                    .read_utf8_lossy(network_value.value.name_ptr.address(), 128)
                    .data_part()?;

                ClassMetadata::NetworkChangeCallback { name }
            },
            "MNetworkVarNames" => unsafe {
                ensure!(
                    !metadata.network_value.is_null(),
                    "null network variable metadata"
                );
                let network_value = mem.read_ptr(metadata.network_value).data()?;
                let var_value = network_value.value.var_value;

                let name = mem
                    .read_utf8_lossy(var_value.name.address(), 128)
                    .data_part()?;

                let type_name = mem
                    .read_utf8_lossy(var_value.type_name.address(), 128)
                    .data_part()?
                    .replace(" ", "");

                ClassMetadata::NetworkVarNames { name, type_name }
            },
            _ => ClassMetadata::Unknown { name },
        };

        acc.push(metadata);

        Ok(acc)
    })
}

fn read_enum_binding(
    mem: &mut impl MemoryView,
    binding_ptr: Pointer64<SchemaEnumBinding>,
) -> Result<Enum> {
    let binding = mem.read_ptr(binding_ptr).data()?;

    let name = mem
        .read_utf8_lossy(binding.name.address(), 128)
        .data_part()?;

    if name.is_empty() {
        bail!("invalid enum name");
    }

    ensure!(
        matches!(binding.alignment, 1 | 2 | 4 | 8),
        "invalid enum alignment"
    );
    let members = read_enum_binding_members(mem, &binding)?;

    Ok(Enum {
        name,
        alignment: binding.alignment,
        size: binding.enumerator_count,
        members,
    })
}

fn read_enum_binding_members(
    mem: &mut impl MemoryView,
    binding: &SchemaEnumBinding,
) -> Result<Vec<EnumMember>> {
    ensure!(
        binding.enumerator_count == 0 || !binding.enumerators.is_null(),
        "null enum members with nonzero count"
    );

    (0..binding.enumerator_count).try_fold(Vec::new(), |mut acc, i| {
        let r#enum = mem.read_ptr(binding.enumerators.at(i as _)).data()?;

        let name = mem
            .read_utf8_lossy(r#enum.name.address(), 128)
            .data_part()?;

        acc.push(EnumMember {
            name,
            value: unsafe { r#enum.value.ulong } as i64,
        });

        Ok(acc)
    })
}

fn read_schema_system<P: Process + MemoryView>(process: &mut P) -> Result<SchemaSystem> {
    let module = process.module_by_name("schemasystem.dll")?;

    let buf = process
        .read_raw(module.base, module.size as _)
        .data_part()?;

    let view = PeView::from_bytes(&buf)?;

    let mut save = [0; 2];

    if !view
        .scanner()
        .finds_code(pattern!("4c8d35${'} 0f2845"), &mut save)
    {
        bail!("outdated schema system pattern");
    }

    let schema_system: SchemaSystem = process.read(module.base + save[1]).data()?;

    if schema_system.registration_count <= 0 {
        bail!("no schema registrations");
    }

    Ok(schema_system)
}

fn read_type_scopes(
    mem: &mut impl MemoryView,
    schema_system: &SchemaSystem,
    report: &mut AnalysisReport,
) -> Result<Vec<TypeScope>> {
    let type_scopes = &schema_system.type_scopes;
    let count = checked_count(type_scopes.count)?;
    ensure!(count > 0, "schema system contains no type scopes");

    let mut scopes = Vec::new();
    for index in 0..count {
        let value = (|| {
            let ptr = type_scopes.element(mem, index)?;
            read_type_scope(mem, ptr, report)
        })();
        if let Some(scope) = report.recover("schemas.scope", &format!("index {index}"), value)? {
            if !scope.classes.is_empty() || !scope.enums.is_empty() {
                scopes.push(scope);
            }
        }
    }
    Ok(scopes)
}

fn read_type_scope(
    mem: &mut impl MemoryView,
    pointer: Pointer64<SchemaSystemTypeScope>,
    report: &mut AnalysisReport,
) -> Result<TypeScope> {
    let scope = mem.read_ptr(pointer).data()?;
    let module_name = bounded_name(&scope.name)?;

    let bindings = report
        .recover(
            "schemas.class_list",
            &module_name,
            scope.class_bindings.elements(mem),
        )?
        .unwrap_or_default();
    let mut classes = Vec::new();
    for ptr in bindings {
        let subject = format!("{module_name} at {:?}", ptr.address());
        if let Some(class) =
            report.recover("schemas.class", &subject, read_class_binding(mem, ptr))?
        {
            if let Some(error) = &class.parent_error {
                report.note(
                    "schemas.parent",
                    &format!("{subject}:{}", class.name),
                    error.clone(),
                );
            }
            classes.push(class);
        }
    }

    let bindings = report
        .recover(
            "schemas.enum_list",
            &module_name,
            scope.enum_bindings.elements(mem),
        )?
        .unwrap_or_default();
    let mut enums = Vec::new();
    for ptr in bindings {
        let subject = format!("{module_name} at {:?}", ptr.address());
        if let Some(value) =
            report.recover("schemas.enum", &subject, read_enum_binding(mem, ptr))?
        {
            enums.push(value);
        }
    }
    debug!(
        "module {module_name:?} contains {} class(es) and {} enum(s)",
        classes.len(),
        enums.len()
    );
    Ok(TypeScope {
        module_name,
        classes,
        enums,
    })
}
