use std::collections::BTreeMap;
use std::fmt::{self, Write};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, ensure};

use chrono::{DateTime, Utc};

use memflow::prelude::v1::*;

use serde_json::json;

use formatter::Formatter;

use crate::analysis::*;

mod buttons;
mod enum_value;
mod formatter;
mod interfaces;
mod offsets;
mod schemas;

enum Item<'a> {
    Buttons(&'a ButtonMap),
    Interfaces(&'a InterfaceMap),
    Offsets(&'a OffsetMap),
    Schemas(&'a SchemaMap),
}

impl<'a> Item<'a> {
    fn write(&self, fmt: &mut Formatter<'a>, file_type: &str) -> fmt::Result {
        match file_type {
            "cs" => self.write_cs(fmt),
            "hpp" => self.write_hpp(fmt),
            "json" => self.write_json(fmt),
            "rs" => self.write_rs(fmt),
            "zig" => self.write_zig(fmt),
            _ => Err(fmt::Error),
        }
    }
}

trait CodeWriter {
    fn write_cs(&self, fmt: &mut Formatter<'_>) -> fmt::Result;
    fn write_hpp(&self, fmt: &mut Formatter<'_>) -> fmt::Result;
    fn write_json(&self, fmt: &mut Formatter<'_>) -> fmt::Result;
    fn write_rs(&self, fmt: &mut Formatter<'_>) -> fmt::Result;
    fn write_zig(&self, fmt: &mut Formatter<'_>) -> fmt::Result;
}

impl<'a> CodeWriter for Item<'a> {
    fn write_cs(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Item::Buttons(buttons) => buttons.write_cs(fmt),
            Item::Interfaces(ifaces) => ifaces.write_cs(fmt),
            Item::Offsets(offsets) => offsets.write_cs(fmt),
            Item::Schemas(schemas) => schemas.write_cs(fmt),
        }
    }

    fn write_hpp(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Item::Buttons(buttons) => buttons.write_hpp(fmt),
            Item::Interfaces(ifaces) => ifaces.write_hpp(fmt),
            Item::Offsets(offsets) => offsets.write_hpp(fmt),
            Item::Schemas(schemas) => schemas.write_hpp(fmt),
        }
    }

    fn write_json(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Item::Buttons(buttons) => buttons.write_json(fmt),
            Item::Interfaces(ifaces) => ifaces.write_json(fmt),
            Item::Offsets(offsets) => offsets.write_json(fmt),
            Item::Schemas(schemas) => schemas.write_json(fmt),
        }
    }

    fn write_rs(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Item::Buttons(buttons) => buttons.write_rs(fmt),
            Item::Interfaces(ifaces) => ifaces.write_rs(fmt),
            Item::Offsets(offsets) => offsets.write_rs(fmt),
            Item::Schemas(schemas) => schemas.write_rs(fmt),
        }
    }

    fn write_zig(&self, fmt: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Item::Buttons(buttons) => buttons.write_zig(fmt),
            Item::Interfaces(ifaces) => ifaces.write_zig(fmt),
            Item::Offsets(offsets) => offsets.write_zig(fmt),
            Item::Schemas(schemas) => schemas.write_zig(fmt),
        }
    }
}

pub struct Output<'a> {
    file_types: &'a [String],
    indent_size: usize,
    out_dir: &'a Path,
    result: &'a AnalysisResult,
    timestamp: DateTime<Utc>,
}

impl<'a> Output<'a> {
    pub fn new(
        file_types: &'a [String],
        indent_size: usize,
        out_dir: &'a Path,
        result: &'a AnalysisResult,
    ) -> Result<Self> {
        ensure!(
            !file_types.is_empty(),
            "at least one output format is required"
        );
        ensure!(indent_size <= 16, "indent size must be between 0 and 16");
        for file_type in file_types {
            ensure!(
                matches!(file_type.as_str(), "cs" | "hpp" | "json" | "rs" | "zig"),
                "unsupported output format: {file_type}"
            );
        }

        if result.report.allow_partial && out_dir.exists() {
            ensure!(
                fs::read_dir(out_dir)?.next().is_none(),
                "--allow-partial requires a new or empty output directory; choose a fresh --output path to avoid stale files"
            );
        }

        Ok(Self {
            file_types,
            indent_size,
            out_dir,
            result,
            timestamp: Utc::now(),
        })
    }

    pub fn dump_all<P: MemoryView + Process>(&self, process: &mut P) -> Result<usize> {
        let items = [
            ("buttons", Item::Buttons(&self.result.buttons)),
            ("interfaces", Item::Interfaces(&self.result.interfaces)),
            ("offsets", Item::Offsets(&self.result.offsets)),
        ];

        let mut report = self.result.report.clone();
        let build_number = report.recover(
            "info",
            "engine2.dll:dwBuildNumber",
            self.read_build_number(process),
        )?;
        let mut files = BTreeMap::new();
        for (file_name, item) in &items {
            self.render_item(file_name, item, &mut files)?;
        }
        for (module_name, (classes, enums)) in &self.result.schemas {
            let map = SchemaMap::from([(module_name.clone(), (classes.clone(), enums.clone()))]);
            self.render_item(&slugify(module_name), &Item::Schemas(&map), &mut files)?;
        }
        let info_path = self.out_dir.join("info.json");
        let errors_path = self.out_dir.join("analysis_errors.json");
        ensure!(
            !files.contains_key(&info_path) && !files.contains_key(&errors_path),
            "schema output collides with a reserved report filename"
        );
        files.insert(
            errors_path,
            serde_json::to_string_pretty(&report_json(&report))?,
        );
        let mut filenames: Vec<_> = files
            .keys()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        filenames.push("info.json".to_string());
        let info = self.render_info(build_number, &report, &filenames)?;

        // Recheck before writing, since partial runs must never mix with old data.
        if report.allow_partial && self.out_dir.exists() {
            ensure!(
                fs::read_dir(self.out_dir)?.next().is_none(),
                "partial output directory is no longer empty"
            );
        }
        fs::create_dir_all(self.out_dir)?;
        // Publish the completion marker last. A failed disk write must not leave
        // an old info.json claiming this mixed set is a complete dump.
        match fs::remove_file(&info_path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        for (path, content) in files {
            fs::write(&path, content)
                .with_context(|| format!("failed to write {}", path.display()))?;
        }
        if let Err(err) = fs::write(&info_path, info) {
            // Best effort: do not leave a partially written completion marker.
            let _ = fs::remove_file(&info_path);
            return Err(err).context("failed to publish info.json");
        }
        Ok(report.issues.len())
    }

    fn read_build_number<P: MemoryView + Process>(&self, process: &mut P) -> Result<u32> {
        let offset = self
            .result
            .offsets
            .get("engine2.dll")
            .and_then(|offsets| offsets.get("dwBuildNumber"))
            .ok_or_else(|| anyhow!("missing engine2.dll dwBuildNumber"))?;
        let module = process.module_by_name("engine2.dll")?;
        process
            .read::<u32>(module.base + *offset)
            .data()
            .context("failed to read build number")
    }

    fn render_info(
        &self,
        build_number: Option<u32>,
        report: &AnalysisReport,
        filenames: &[String],
    ) -> Result<String> {
        Ok(serde_json::to_string_pretty(&json!({
            "timestamp": self.timestamp.to_rfc3339(),
            "build_number": build_number,
            "allow_partial": report.allow_partial,
            "analysis_complete": report.issues.is_empty(),
            "issue_count": report.issues.len(),
            "issues_file": "analysis_errors.json",
            "files": filenames,
        }))?)
    }

    fn render_item(
        &self,
        file_name: &str,
        item: &Item,
        files: &mut BTreeMap<PathBuf, String>,
    ) -> Result<()> {
        let mut seen_formats = std::collections::HashSet::new();
        for file_type in self.file_types {
            if !seen_formats.insert(file_type) {
                continue;
            }
            let mut out = String::new();
            let mut fmt = Formatter::new(&mut out, self.indent_size);
            if file_type != "json" {
                self.write_banner(&mut fmt)?;
            }
            item.write(&mut fmt, file_type)?;
            let path = self.out_dir.join(format!("{file_name}.{file_type}"));
            ensure!(
                !files.contains_key(&path),
                "output filename collision: {}",
                path.display()
            );
            files.insert(path, out);
        }
        Ok(())
    }

    fn write_banner(&self, fmt: &mut Formatter<'_>) -> Result<()> {
        writeln!(fmt, "// Generated using https://github.com/a2x/cs2-dumper")?;
        writeln!(fmt, "// {}\n", self.timestamp)?;
        if self.result.report.allow_partial || !self.result.report.issues.is_empty() {
            writeln!(
                fmt,
                "// Output may be incomplete. Check info.json and analysis_errors.json."
            )?;
        }

        Ok(())
    }
}

fn report_json(report: &AnalysisReport) -> serde_json::Value {
    let issues: Vec<_> = report
        .issues
        .iter()
        .map(|issue| {
            json!({
                "stage": issue.stage,
                "subject": issue.subject,
                "message": issue.message,
            })
        })
        .collect();
    json!({
        "allow_partial": report.allow_partial,
        "analysis_complete": issues.is_empty(),
        "issues": issues,
    })
}

#[inline]
fn slugify(input: &str) -> String {
    input.replace(|c: char| !c.is_alphanumeric(), "_")
}

#[inline]
fn zig_ident(input: &str) -> String {
    if is_zig_identifier(input) && !is_zig_keyword(input) {
        input.to_string()
    } else {
        let escaped = input.replace('\\', "\\\\").replace('"', "\\\"");

        format!("@\"{}\"", escaped)
    }
}

#[inline]
fn is_zig_identifier(input: &str) -> bool {
    let mut chars = input.chars();

    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }

    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

#[inline]
fn is_zig_keyword(input: &str) -> bool {
    matches!(
        input,
        "addrspace"
            | "align"
            | "allowzero"
            | "and"
            | "anyframe"
            | "anytype"
            | "asm"
            | "async"
            | "await"
            | "break"
            | "callconv"
            | "catch"
            | "comptime"
            | "const"
            | "continue"
            | "defer"
            | "else"
            | "enum"
            | "errdefer"
            | "error"
            | "export"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "inline"
            | "linksection"
            | "noalias"
            | "noinline"
            | "nosuspend"
            | "null"
            | "opaque"
            | "or"
            | "orelse"
            | "packed"
            | "pub"
            | "resume"
            | "return"
            | "struct"
            | "suspend"
            | "switch"
            | "test"
            | "threadlocal"
            | "true"
            | "try"
            | "union"
            | "unreachable"
            | "usingnamespace"
            | "var"
            | "volatile"
            | "while"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_analysis() -> AnalysisResult {
        AnalysisResult {
            buttons: BTreeMap::new(),
            interfaces: BTreeMap::new(),
            offsets: BTreeMap::new(),
            schemas: BTreeMap::new(),
            report: AnalysisReport::default(),
        }
    }

    #[test]
    fn missing_build_number_is_explicitly_null_in_partial_output() {
        let result = empty_analysis();
        let formats = ["json".into()];
        let output = Output::new(&formats, 4, Path::new("output"), &result).unwrap();
        let mut report = AnalysisReport::new(true);
        let build = report
            .recover::<u32>("info", "dwBuildNumber", Err(anyhow!("read failed")))
            .unwrap();
        let text = output
            .render_info(build, &report, &["offsets.json".into()])
            .unwrap();
        let info: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(info["build_number"].is_null());
        assert_eq!(info["analysis_complete"], false);
        assert_eq!(info["issue_count"], 1);
        let errors = report_json(&report);
        assert_eq!(errors["issues"][0]["subject"], "dwBuildNumber");
        assert!(
            errors["issues"][0]["message"]
                .as_str()
                .unwrap()
                .contains("read failed")
        );
    }

    #[test]
    fn optional_omissions_also_mark_output_incomplete() {
        let result = empty_analysis();
        let formats = ["json".into()];
        let output = Output::new(&formats, 4, Path::new("output"), &result).unwrap();
        let mut report = AnalysisReport::default();
        report.note("schemas.parent", "Child", "unresolved parent");
        let text = output.render_info(Some(1234), &report, &[]).unwrap();
        let info: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(info["analysis_complete"], false);
        assert_eq!(info["build_number"], 1234);
        assert_eq!(report_json(&report)["analysis_complete"], false);
    }

    #[test]
    fn partial_output_rejects_nonempty_directories_without_deleting_files() {
        let path = std::env::temp_dir().join(format!(
            "cs2-dumper-output-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        let formats = ["json".into()];
        let mut result = empty_analysis();
        result.report.allow_partial = true;
        assert!(Output::new(&formats, 4, &path, &result).is_ok());
        fs::write(path.join("old.json"), "old data").unwrap();
        assert!(Output::new(&formats, 4, &path, &result).is_err());
        assert_eq!(
            fs::read_to_string(path.join("old.json")).unwrap(),
            "old data"
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn validates_output_options() {
        let result = empty_analysis();
        let path = Path::new("unused-test-output");
        assert!(Output::new(&["txt".into()], 4, path, &result).is_err());
        assert!(Output::new(&["../bad".into()], 4, path, &result).is_err());
        assert!(Output::new(&[], 4, path, &result).is_err());
        assert!(Output::new(&["json".into()], usize::MAX, path, &result).is_err());
    }

    #[test]
    fn rendering_detects_collisions_and_deduplicates_formats() {
        let result = empty_analysis();
        let formats = ["json".into(), "json".into()];
        let output = Output::new(&formats, 4, Path::new("output"), &result).unwrap();
        let mut files = BTreeMap::new();
        let item = Item::Buttons(&result.buttons);
        output.render_item("buttons", &item, &mut files).unwrap();
        assert_eq!(files.len(), 1);
        assert!(serde_json::from_str::<serde_json::Value>(files.values().next().unwrap()).is_ok());
        assert!(output.render_item("buttons", &item, &mut files).is_err());
    }

    #[test]
    fn unknown_format_returns_error_instead_of_panicking() {
        let result = empty_analysis();
        let mut text = String::new();
        let mut fmt = Formatter::new(&mut text, 4);
        assert!(
            Item::Buttons(&result.buttons)
                .write(&mut fmt, "txt")
                .is_err()
        );
    }
}
