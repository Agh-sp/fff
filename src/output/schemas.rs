use std::collections::{BTreeMap, HashSet};
use std::fmt::{self, Write};

use heck::{AsPascalCase, AsSnakeCase};

use serde_json::json;

use super::{CodeWriter, Formatter, SchemaMap, slugify, zig_ident};

use super::enum_value::{format_enum_member_value, unsigned_enum_value};
use crate::analysis::{Class, ClassMetadata};

impl CodeWriter for SchemaMap {
    fn write_cs(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        fmt.block("namespace CS2Dumper.Schemas", false, |fmt| {
            for (module_name, (classes, enums)) in self {
                writeln!(fmt, "// Module: {}", module_name)?;
                writeln!(fmt, "// Class count: {}", classes.len())?;
                writeln!(fmt, "// Enum count: {}", enums.len())?;

                fmt.block(
                    &format!("public static class {}", AsPascalCase(slugify(module_name))),
                    false,
                    |fmt| {
                        for enum_ in enums {
                            let type_name = match enum_.alignment {
                                1 => "byte",
                                2 => "ushort",
                                4 => "uint",
                                8 => "ulong",
                                _ => continue,
                            };

                            writeln!(fmt, "// Alignment: {}", enum_.alignment)?;
                            writeln!(fmt, "// Member count: {}", enum_.size)?;

                            fmt.block(
                                &format!("public enum {} : {}", slugify(&enum_.name), type_name),
                                false,
                                |fmt| {
                                    let members = enum_
                                        .members
                                        .iter()
                                        .map(|member| {
                                            let formatted_value = format_enum_member_value(
                                                member.value,
                                                enum_.alignment,
                                            );

                                            format!("{} = {}", member.name, formatted_value)
                                        })
                                        .collect::<Vec<_>>()
                                        .join(",\n");

                                    writeln!(fmt, "{}", members)
                                },
                            )?;
                        }

                        for class in classes {
                            let parent_name = parent_label(class);

                            writeln!(fmt, "// Parent: {}", parent_name)?;
                            writeln!(fmt, "// Field count: {}", class.fields.len())?;

                            write_metadata(fmt, &class.metadata)?;

                            fmt.block(
                                &format!("public static class {}", slugify(&class.name)),
                                false,
                                |fmt| {
                                    for field in &class.fields {
                                        writeln!(
                                            fmt,
                                            "public const nint {} = {:#X}; // {}",
                                            field.name, field.offset, field.type_name
                                        )?;
                                    }

                                    Ok(())
                                },
                            )?;
                        }

                        Ok(())
                    },
                )?;
            }

            Ok(())
        })
    }

    fn write_hpp(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        writeln!(fmt, "#pragma once\n")?;
        writeln!(fmt, "#include <cstddef>")?;
        writeln!(fmt, "#include <cstdint>\n")?;

        fmt.block("namespace cs2_dumper", false, |fmt| {
            fmt.block("namespace schemas", false, |fmt| {
                for (module_name, (classes, enums)) in self {
                    writeln!(fmt, "// Module: {}", module_name)?;
                    writeln!(fmt, "// Class count: {}", classes.len())?;
                    writeln!(fmt, "// Enum count: {}", enums.len())?;

                    fmt.block(
                        &format!("namespace {}", AsSnakeCase(slugify(module_name))),
                        false,
                        |fmt| {
                            for enum_ in enums {
                                let type_name = match enum_.alignment {
                                    1 => "uint8_t",
                                    2 => "uint16_t",
                                    4 => "uint32_t",
                                    8 => "uint64_t",
                                    _ => continue,
                                };

                                writeln!(fmt, "// Alignment: {}", enum_.alignment)?;
                                writeln!(fmt, "// Member count: {}", enum_.size)?;

                                fmt.block(
                                    &format!("enum class {} : {}", slugify(&enum_.name), type_name),
                                    true,
                                    |fmt| {
                                        let members = enum_
                                            .members
                                            .iter()
                                            .map(|member| {
                                                let formatted_value = format_enum_member_value(
                                                    member.value,
                                                    enum_.alignment,
                                                );

                                                format!("{} = {}", member.name, formatted_value)
                                            })
                                            .collect::<Vec<_>>()
                                            .join(",\n");

                                        writeln!(fmt, "{}", members)
                                    },
                                )?;
                            }

                            for class in classes {
                                let parent_name = parent_label(class);

                                writeln!(fmt, "// Parent: {}", parent_name)?;
                                writeln!(fmt, "// Field count: {}", class.fields.len())?;

                                write_metadata(fmt, &class.metadata)?;

                                fmt.block(
                                    &format!("namespace {}", slugify(&class.name)),
                                    false,
                                    |fmt| {
                                        for field in &class.fields {
                                            writeln!(
                                                fmt,
                                                "constexpr std::ptrdiff_t {} = {:#X}; // {}",
                                                field.name, field.offset, field.type_name
                                            )?;
                                        }

                                        Ok(())
                                    },
                                )?;
                            }

                            Ok(())
                        },
                    )?;
                }

                Ok(())
            })
        })
    }

    fn write_json(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        let content: BTreeMap<_, _> = self
            .iter()
            .map(|(module_name, (classes, enums))| {
                let classes: BTreeMap<_, _> = classes
                    .iter()
                    .map(|class| {
                        let fields: BTreeMap<_, _> = class
                            .fields
                            .iter()
                            .map(|field| (&field.name, field.offset))
                            .collect();

                        let metadata: Vec<_> = class
                            .metadata
                            .iter()
                            .map(|metadata| match metadata {
                                ClassMetadata::NetworkChangeCallback { name } => json!({
                                    "type": "NetworkChangeCallback",
                                    "name": name,
                                }),
                                ClassMetadata::NetworkVarNames { name, type_name } => json!({
                                    "type": "NetworkVarNames",
                                    "name": name,
                                    "type_name": type_name,
                                }),
                                ClassMetadata::Unknown { name } => json!({
                                    "type": "Unknown",
                                    "name": name,
                                }),
                            })
                            .collect();

                        let mut entry = json!({
                            "parent": class.parent_name,
                            "fields": fields,
                            "metadata": metadata
                        });
                        // null without an error means no parent; null with an
                        // error means inheritance is unknown, not absent.
                        if let Some(error) = &class.parent_error {
                            entry["parent_error"] = json!(error);
                        }
                        (slugify(&class.name), entry)
                    })
                    .collect();

                let enums: BTreeMap<_, _> = enums
                    .iter()
                    .map(|enum_| {
                        let members: BTreeMap<_, _> = enum_
                            .members
                            .iter()
                            .map(|member| (&member.name, member.value))
                            .collect();

                        let type_name = match enum_.alignment {
                            1 => "uint8",
                            2 => "uint16",
                            4 => "uint32",
                            8 => "uint64",
                            _ => "unknown",
                        };

                        (
                            slugify(&enum_.name),
                            json!({
                                "alignment": enum_.alignment,
                                "type": type_name,
                                "members": members,
                            }),
                        )
                    })
                    .collect();

                (
                    module_name,
                    json!({
                        "classes": classes,
                        "enums": enums,
                    }),
                )
            })
            .collect();

        fmt.write_str(&serde_json::to_string_pretty(&content).unwrap())
    }

    fn write_rs(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        writeln!(
            fmt,
            "#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, unused)]\n"
        )?;

        fmt.block("pub mod cs2_dumper", false, |fmt| {
            fmt.block("pub mod schemas", false, |fmt| {
                for (module_name, (classes, enums)) in self {
                    writeln!(fmt, "// Module: {}", module_name)?;
                    writeln!(fmt, "// Class count: {}", classes.len())?;
                    writeln!(fmt, "// Enum count: {}", enums.len())?;

                    fmt.block(
                        &format!("pub mod {}", AsSnakeCase(slugify(module_name))),
                        false,
                        |fmt| {
                            for enum_ in enums {
                                let type_name = match enum_.alignment {
                                    1 => "u8",
                                    2 => "u16",
                                    4 => "u32",
                                    8 => "u64",
                                    _ => continue,
                                };

                                writeln!(fmt, "// Alignment: {}", enum_.alignment)?;
                                writeln!(fmt, "// Member count: {}", enum_.size)?;

                                fmt.block(
                                    &format!(
                                        "#[repr({})]\npub enum {}",
                                        type_name,
                                        slugify(&enum_.name),
                                    ),
                                    false,
                                    |fmt| {
                                        let mut used_values = HashSet::new();

                                        let members = enum_
                                            .members
                                            .iter()
                                            .filter_map(|member| {
                                                // Skip duplicate values.
                                                if !used_values.insert(unsigned_enum_value(
                                                    member.value,
                                                    enum_.alignment,
                                                )) {
                                                    return None;
                                                }

                                                let formatted_value = format_enum_member_value(
                                                    member.value,
                                                    enum_.alignment,
                                                );

                                                Some(format!(
                                                    "{} = {}",
                                                    member.name, formatted_value
                                                ))
                                            })
                                            .collect::<Vec<_>>()
                                            .join(",\n");

                                        writeln!(fmt, "{}", members)
                                    },
                                )?;
                            }

                            for class in classes {
                                let parent_name = parent_label(class);

                                writeln!(fmt, "// Parent: {}", parent_name)?;
                                writeln!(fmt, "// Field count: {}", class.fields.len())?;

                                write_metadata(fmt, &class.metadata)?;

                                fmt.block(
                                    &format!("pub mod {}", slugify(&class.name)),
                                    false,
                                    |fmt| {
                                        for field in &class.fields {
                                            writeln!(
                                                fmt,
                                                "pub const {}: usize = {:#X}; // {}",
                                                field.name, field.offset, field.type_name
                                            )?;
                                        }

                                        Ok(())
                                    },
                                )?;
                            }

                            Ok(())
                        },
                    )?;
                }

                Ok(())
            })
        })
    }

    fn write_zig(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        fmt.block("pub const cs2_dumper = struct", true, |fmt| {
            fmt.block("pub const schemas = struct", true, |fmt| {
                for (module_name, (classes, enums)) in self {
                    writeln!(fmt, "// Module: {}", module_name)?;
                    writeln!(fmt, "// Class count: {}", classes.len())?;
                    writeln!(fmt, "// Enum count: {}", enums.len())?;

                    let module_name = zig_ident(&AsSnakeCase(slugify(module_name)).to_string());

                    fmt.block(
                        &format!("pub const {} = struct", module_name),
                        true,
                        |fmt| {
                            for enum_ in enums {
                                let type_name = match enum_.alignment {
                                    1 => "u8",
                                    2 => "u16",
                                    4 => "u32",
                                    8 => "u64",
                                    _ => continue,
                                };

                                writeln!(fmt, "// Alignment: {}", enum_.alignment)?;
                                writeln!(fmt, "// Member count: {}", enum_.size)?;

                                let enum_name = zig_ident(&slugify(&enum_.name));

                                fmt.block(
                                    &format!("pub const {} = enum({})", enum_name, type_name),
                                    true,
                                    |fmt| {
                                        let mut used_values = HashSet::new();

                                        let members = enum_
                                            .members
                                            .iter()
                                            .filter_map(|member| {
                                                // Skip duplicate values.
                                                if !used_values.insert(unsigned_enum_value(
                                                    member.value,
                                                    enum_.alignment,
                                                )) {
                                                    return None;
                                                }

                                                let formatted_value = format_enum_member_value(
                                                    member.value,
                                                    enum_.alignment,
                                                );

                                                Some(format!(
                                                    "{} = {}",
                                                    zig_ident(&member.name),
                                                    formatted_value
                                                ))
                                            })
                                            .collect::<Vec<_>>()
                                            .join(",\n");

                                        writeln!(fmt, "{}", members)
                                    },
                                )?;
                            }

                            for class in classes {
                                let parent_name = parent_label(class);

                                writeln!(fmt, "// Parent: {}", parent_name)?;
                                writeln!(fmt, "// Field count: {}", class.fields.len())?;

                                write_metadata(fmt, &class.metadata)?;

                                let class_name = zig_ident(&slugify(&class.name));

                                fmt.block(
                                    &format!("pub const {} = struct", class_name),
                                    true,
                                    |fmt| {
                                        for field in &class.fields {
                                            writeln!(
                                                fmt,
                                                "pub const {}: usize = {:#X}; // {}",
                                                zig_ident(&field.name),
                                                field.offset,
                                                field.type_name
                                            )?;
                                        }

                                        Ok(())
                                    },
                                )?;
                            }

                            Ok(())
                        },
                    )?;
                }

                Ok(())
            })
        })
    }
}

fn parent_label(class: &Class) -> String {
    if class.parent_error.is_some() {
        "UNRESOLVED (see parent_error in JSON or the log)".to_string()
    } else {
        class
            .parent_name
            .as_deref()
            .map(slugify)
            .unwrap_or_else(|| "None".to_string())
    }
}

fn write_metadata(fmt: &mut Formatter<'_>, metadata: &[ClassMetadata]) -> fmt::Result {
    if metadata.is_empty() {
        return Ok(());
    }

    writeln!(fmt, "//")?;
    writeln!(fmt, "// Metadata:")?;

    for metadata in metadata {
        match metadata {
            ClassMetadata::NetworkChangeCallback { name } => {
                writeln!(fmt, "// NetworkChangeCallback: {}", name)?;
            }
            ClassMetadata::NetworkVarNames { name, type_name } => {
                writeln!(fmt, "// NetworkVarNames: {} ({})", name, type_name)?;
            }
            ClassMetadata::Unknown { name } => {
                writeln!(fmt, "// {}", name)?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{Enum, EnumMember};

    fn parent_fixture() -> SchemaMap {
        let class = |name: &str, parent: Option<&str>, error: Option<&str>| Class {
            name: name.into(),
            module_name: "test.dll".into(),
            parent_name: parent.map(str::to_string),
            parent_error: error.map(str::to_string),
            fields: vec![crate::analysis::ClassField {
                name: "m_value".into(),
                type_name: "int32".into(),
                offset: 16,
            }],
            metadata: vec![],
        };
        SchemaMap::from([(
            "test.dll".into(),
            (
                vec![
                    class("Root", None, None),
                    class("Child", Some("Root"), None),
                    class("Unresolved", None, Some("invalid parent class name")),
                ],
                vec![],
            ),
        )])
    }

    #[test]
    fn json_distinguishes_absent_and_unresolved_parents_without_losing_fields() {
        let schemas = parent_fixture();
        let mut text = String::new();
        schemas
            .write_json(&mut Formatter::new(&mut text, 4))
            .unwrap();
        let output: serde_json::Value = serde_json::from_str(&text).unwrap();
        let classes = &output["test.dll"]["classes"];
        assert!(classes["Root"]["parent"].is_null());
        assert!(classes["Root"].get("parent_error").is_none());
        assert_eq!(classes["Child"]["parent"], "Root");
        assert!(classes["Child"].get("parent_error").is_none());
        assert!(classes["Unresolved"]["parent"].is_null());
        assert_eq!(
            classes["Unresolved"]["parent_error"],
            "invalid parent class name"
        );
        assert_eq!(classes["Unresolved"]["fields"]["m_value"], 16);
    }

    #[test]
    fn generated_code_marks_unresolved_parent_metadata() {
        let schemas = parent_fixture();
        for extension in ["cs", "hpp", "rs", "zig"] {
            let mut text = String::new();
            let mut fmt = Formatter::new(&mut text, 4);
            super::super::Item::Schemas(&schemas)
                .write(&mut fmt, extension)
                .unwrap();
            assert!(
                text.contains("// Parent: UNRESOLVED"),
                "{extension}: {text}"
            );
            assert!(text.contains("// Parent: None"));
            assert!(text.contains("// Parent: Root"));
            assert_eq!(text.matches("m_value").count(), 3);
        }
    }

    #[test]
    fn old_class_serializations_default_to_no_parent_error() {
        let class: Class = serde_json::from_value(json!({
            "name": "Root", "module_name": "test.dll", "parent_name": null,
            "fields": [], "metadata": [],
        }))
        .unwrap();
        assert!(class.parent_error.is_none());
        assert!(
            serde_json::to_value(&class)
                .unwrap()
                .get("parent_error")
                .is_none()
        );
    }

    #[test]
    fn writers_preserve_high_bits() {
        let schemas = SchemaMap::from([(
            "test.dll".into(),
            (
                vec![],
                vec![Enum {
                    name: "Flags".into(),
                    alignment: 4,
                    size: 1,
                    members: vec![EnumMember {
                        name: "HighBit".into(),
                        value: 0x80000000,
                    }],
                }],
            ),
        )]);
        for extension in ["cs", "hpp", "rs", "zig"] {
            let mut text = String::new();
            let mut fmt = Formatter::new(&mut text, 4);
            super::super::Item::Schemas(&schemas)
                .write(&mut fmt, extension)
                .unwrap();
            assert!(text.contains("HighBit = 0x80000000"), "{extension}: {text}");
        }
    }

    #[test]
    fn rust_and_zig_deduplicate_wrapped_values() {
        let schemas = SchemaMap::from([(
            "test.dll".into(),
            (
                vec![],
                vec![Enum {
                    name: "Aliases".into(),
                    alignment: 1,
                    size: 2,
                    members: vec![
                        EnumMember {
                            name: "First".into(),
                            value: -1,
                        },
                        EnumMember {
                            name: "Alias".into(),
                            value: 255,
                        },
                    ],
                }],
            ),
        )]);
        for extension in ["rs", "zig"] {
            let mut text = String::new();
            let mut fmt = Formatter::new(&mut text, 4);
            super::super::Item::Schemas(&schemas)
                .write(&mut fmt, extension)
                .unwrap();
            assert!(text.contains("First = 0xFF"));
            assert!(!text.contains("Alias ="));
        }
    }
}
