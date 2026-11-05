use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};

use log::debug;

use memflow::prelude::v1::*;

use pelite::pattern;
use pelite::pe64::{Pe, PeView};

use serde::{Deserialize, Serialize};

use crate::memory::validation::{bounded_name, checked_count};
use crate::source2::*;

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

pub fn schemas<P: Process + MemoryView>(process: &mut P) -> Result<SchemaMap> {
    let schema_system = read_schema_system(process)?;
    let type_scopes = read_type_scopes(process, &schema_system)?;

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

    let parent_name = if let Some(ptr) = binding.base_classes.non_null() {
        let base_class = mem.read_ptr(ptr).data()?;
        let parent_class = mem.read_ptr(base_class.class).data()?;
        let name = mem
            .read_utf8_lossy(parent_class.name.address(), 128)
            .data_part()?;
        ensure!(!name.is_empty(), "invalid parent class name");
        Some(name)
    } else {
        None
    };

    let fields = read_class_binding_fields(mem, &binding)?;
    let metadata = read_class_binding_metadata(mem, &binding)?;

    Ok(Class {
        name,
        module_name,
        parent_name,
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
) -> Result<Vec<TypeScope>> {
    let type_scopes = &schema_system.type_scopes;
    let count = checked_count(type_scopes.count)?;
    ensure!(count > 0, "schema system contains no type scopes");

    (0..count).try_fold(Vec::new(), |mut acc, i| {
        let type_scope_ptr = type_scopes.element(mem, i as _)?;
        let type_scope = mem.read_ptr(type_scope_ptr).data()?;

        let module_name = bounded_name(&type_scope.name)?;

        let classes: Vec<_> = type_scope
            .class_bindings
            .elements(mem)?
            .iter()
            .map(|ptr| {
                read_class_binding(mem, *ptr).with_context(|| {
                    format!("invalid class in {module_name} at {:?}", ptr.address())
                })
            })
            .collect::<Result<_>>()?;

        let enums: Vec<_> = type_scope
            .enum_bindings
            .elements(mem)?
            .iter()
            .map(|ptr| {
                read_enum_binding(mem, *ptr).with_context(|| {
                    format!("invalid enum in {module_name} at {:?}", ptr.address())
                })
            })
            .collect::<Result<_>>()?;

        if classes.is_empty() && enums.is_empty() {
            return Ok(acc);
        }

        debug!(
            "module \"{}\" contains {} class(es) and {} enum(s)",
            module_name,
            classes.len(),
            enums.len(),
        );

        acc.push(TypeScope {
            module_name,
            classes,
            enums,
        });

        Ok(acc)
    })
}
