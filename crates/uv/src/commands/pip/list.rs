use std::cmp::max;
use std::fmt::Write;

use anstream::println;
use anyhow::Result;
use itertools::Itertools;
use owo_colors::OwoColorize;
use serde::Serialize;
use unicode_width::UnicodeWidthStr;

use uv_cache::Cache;
use uv_cli::ListFormat;
use uv_distribution_types::{Diagnostic, InstalledDist, Name};
use uv_fs::Simplified;
use uv_installer::SitePackages;
use uv_normalize::PackageName;
use uv_python::PythonRequest;
use uv_python::{EnvironmentPreference, PythonEnvironment};

use crate::commands::pip::operations::report_target_environment;
use crate::commands::ExitStatus;
use crate::printer::Printer;

/// Enumerate the installed packages in the current environment.
#[allow(clippy::fn_params_excessive_bools)]
pub(crate) fn pip_list(
    editable: Option<bool>,
    exclude: &[PackageName],
    format: &ListFormat,
    strict: bool,
    python: Option<&str>,
    system: bool,
    cache: &Cache,
    printer: Printer,
) -> Result<ExitStatus> {
    // Detect the current Python interpreter.
    let environment = PythonEnvironment::find(
        &python.map(PythonRequest::parse).unwrap_or_default(),
        EnvironmentPreference::from_system_flag(system, false),
        cache,
    )?;

    report_target_environment(&environment, cache, printer)?;

    // Build the installed index.
    let site_packages = SitePackages::from_environment(&environment)?;

    // Filter if `--editable` is specified; always sort by name.
    let results = site_packages
        .iter()
        .filter(|dist| editable.is_none() || editable == Some(dist.is_editable()))
        .filter(|dist| !exclude.contains(dist.name()))
        .sorted_unstable_by(|a, b| a.name().cmp(b.name()).then(a.version().cmp(b.version())))
        .collect_vec();

    match format {
        ListFormat::Json => {
            let rows = results.iter().copied().map(Entry::from).collect_vec();
            let output = serde_json::to_string(&rows)?;
            println!("{output}");
        }
        ListFormat::Columns if results.is_empty() => {}
        ListFormat::Columns => {
            // The package name and version are always present.
            let mut columns = vec![
                Column {
                    header: String::from("Package"),
                    rows: results
                        .iter()
                        .copied()
                        .map(|dist| dist.name().to_string())
                        .collect_vec(),
                },
                Column {
                    header: String::from("Version"),
                    rows: results
                        .iter()
                        .map(|dist| dist.version().to_string())
                        .collect_vec(),
                },
            ];

            // Editable column is only displayed if at least one editable package is found.
            if results.iter().copied().any(InstalledDist::is_editable) {
                columns.push(Column {
                    header: String::from("Editable project location"),
                    rows: results
                        .iter()
                        .map(|dist| dist.as_editable())
                        .map(|url| {
                            url.map(|url| {
                                url.to_file_path().unwrap().simplified_display().to_string()
                            })
                            .unwrap_or_default()
                        })
                        .collect_vec(),
                });
            }

            for elems in MultiZip(columns.iter().map(Column::fmt).collect_vec()) {
                println!("{}", elems.join(" ").trim_end());
            }
        }
        ListFormat::Freeze if results.is_empty() => {}
        ListFormat::Freeze => {
            for dist in &results {
                println!("{}=={}", dist.name().bold(), dist.version());
            }
        }
    }

    // Validate that the environment is consistent.
    if strict {
        // Determine the markers to use for resolution.
        let markers = environment.interpreter().resolver_marker_environment();

        for diagnostic in site_packages.diagnostics(&markers)? {
            writeln!(
                printer.stderr(),
                "{}{} {}",
                "warning".yellow().bold(),
                ":".bold(),
                diagnostic.message().bold()
            )?;
        }
    }

    Ok(ExitStatus::Success)
}

/// An entry in a JSON list of installed packages.
#[derive(Debug, Serialize)]
struct Entry {
    name: String,
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    editable_project_location: Option<String>,
}

impl From<&InstalledDist> for Entry {
    fn from(dist: &InstalledDist) -> Self {
        Self {
            name: dist.name().to_string(),
            version: dist.version().to_string(),
            editable_project_location: dist
                .as_editable()
                .map(|url| url.to_file_path().unwrap().simplified_display().to_string()),
        }
    }
}

#[derive(Debug)]
struct Column {
    /// The header of the column.
    header: String,
    /// The rows of the column.
    rows: Vec<String>,
}

impl<'a> Column {
    /// Return the width of the column.
    fn max_width(&self) -> usize {
        max(
            self.header.width(),
            self.rows.iter().map(|f| f.width()).max().unwrap_or(0),
        )
    }

    /// Return an iterator of the column, with the header and rows formatted to the maximum width.
    fn fmt(&'a self) -> impl Iterator<Item = String> + 'a {
        let max_width = self.max_width();
        let header = vec![
            format!("{0:width$}", self.header, width = max_width),
            format!("{:-^width$}", "", width = max_width),
        ];

        header
            .into_iter()
            .chain(self.rows.iter().map(move |f| format!("{f:max_width$}")))
    }
}

/// Zip an unknown number of iterators.
/// Combination of [`itertools::multizip`] and [`itertools::izip`].
#[derive(Debug)]
struct MultiZip<T>(Vec<T>);

impl<T> Iterator for MultiZip<T>
where
    T: Iterator,
{
    type Item = Vec<T::Item>;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.iter_mut().map(Iterator::next).collect()
    }
}
