use crate::report::{display_value, safe_label};
use chrono::{DateTime, Utc};
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table, TableComponent, presets};
use console::{Style, Term};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::IsTerminal;

/// Terminal capabilities belong to the output stream, not the process as a whole.
struct Presentation {
    width: u16,
    color: bool,
    unicode: bool,
}

impl Presentation {
    fn stdout() -> Self {
        let tty = std::io::stdout().is_terminal();
        let dumb = std::env::var("TERM").is_ok_and(|term| term == "dumb");
        let width = std::env::var("COLUMNS")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|width| *width > 0)
            .unwrap_or_else(|| if tty { Term::stdout().size().1 } else { 100 });
        Self {
            width: width.max(1),
            color: tty && !crate::terminal_color_disabled() && console::colors_enabled(),
            unicode: tty && !dumb,
        }
    }

    fn title(&self, title: &str) -> String {
        Style::new().bold().apply_to(safe_label(title)).force_styling(self.color).to_string()
    }

    fn status(&self, text: &str) -> Cell {
        let cell = Cell::new(safe_label(text));
        if !self.color {
            return cell;
        }
        match text {
            "success" | "written" | "available" | "public" | "true" => cell.fg(Color::Green),
            "failed" | "upstream_failed" | "timeout" | "false" => cell.fg(Color::Red),
            "missing_credentials"
            | "ambiguous"
            | "stale"
            | "retired"
            | "cancelled"
            | "network_restricted"
            | "no_data" => cell.fg(Color::Yellow),
            _ => cell,
        }
    }

    fn table(&self, headers: &[&str], rows: Vec<Vec<Cell>>) -> String {
        if rows.is_empty() {
            return String::new();
        }
        // A narrow terminal cannot usefully fit six columns. Keep every value
        // visible in labeled records rather than squeezing words into fragments.
        if self.width < 60 {
            let mut table = Table::new();
            table
                .load_preset(presets::NOTHING)
                .set_content_arrangement(ContentArrangement::Dynamic)
                .set_width(self.width);
            for row in rows {
                let content = headers
                    .iter()
                    .zip(row)
                    .map(|(header, cell)| format!("{header}: {}", cell.content()))
                    .collect::<Vec<_>>()
                    .join("\n");
                table.add_row(vec![Cell::new(content)]);
                table.add_row(vec![Cell::new("")]);
            }
            for column in table.column_iter_mut() {
                column.set_padding((0, 0));
            }
            return table.to_string().trim_end().to_owned();
        }
        let mut table = Table::new();
        table.load_preset(presets::NOTHING);
        let rule = if self.unicode { '─' } else { '-' };
        table.set_style(TableComponent::HeaderLines, rule);
        table.set_style(TableComponent::MiddleHeaderIntersections, rule);
        table.set_content_arrangement(ContentArrangement::Dynamic).set_width(self.width);
        if self.color {
            table.enforce_styling();
        }
        table.set_header(headers.iter().map(|label| {
            let cell = Cell::new(*label);
            if self.color { cell.add_attribute(Attribute::Bold) } else { cell }
        }));
        for row in rows {
            table.add_row(row);
        }
        table.to_string()
    }
}

pub(crate) fn error(message: &str) -> String {
    let color = std::io::stderr().is_terminal()
        && !crate::terminal_color_disabled()
        && console::colors_enabled_stderr();
    let label = Style::new().red().bold().apply_to("radiust: error:").force_styling(color);
    format!("{label} {}", safe_label(message))
}

pub(crate) fn render(value: &Value, hint: Option<&str>, verbose: bool) -> String {
    let presentation = Presentation::stdout();
    render_with(value, hint, verbose, &presentation)
}

fn render_with(value: &Value, hint: Option<&str>, verbose: bool, p: &Presentation) -> String {
    let command = value["command"].as_str().or(hint).unwrap_or("result");
    let heading = if command == "cache" {
        format!("CACHE {}", value["operation"].as_str().unwrap_or("status").to_ascii_uppercase())
    } else {
        command.to_ascii_uppercase()
    };
    let mut sections = vec![p.title(&heading)];
    if let Some(query) = value.get("query").filter(|query| query.is_object()) {
        let mut parts = Vec::new();
        for key in ["source", "sources", "product", "stations", "at", "start", "end", "base_time"] {
            if let Some(field) = query.get(key).filter(|field| !field.is_null()) {
                if field.as_array().is_some_and(Vec::is_empty) {
                    continue;
                }
                parts.push(format!("{key}: {}", display_value(field)));
            }
        }
        if query["latest"] == true {
            parts.push("latest".into());
        }
        if !query["max_age"].is_null() {
            parts.push(format!("max age: {}s", display_value(&query["max_age"])));
        }
        sections.push(parts.join("  |  "));
    }
    if let Some(items) = value["items"].as_array() {
        if matches!(command, "discover" | "download") {
            let counts = value["counts"]
                .as_object()
                .map(|counts| {
                    counts
                        .iter()
                        .filter(|(key, number)| {
                            !matches!(key.as_str(), "total" | "items")
                                && number.as_u64().unwrap_or(0) > 0
                        })
                        .map(|(key, number)| format!("{number} {key}"))
                        .collect::<Vec<_>>()
                        .join("  |  ")
                })
                .unwrap_or_default();
            sections.push(format!(
                "Items: {}{}",
                items.len(),
                if counts.is_empty() { String::new() } else { format!("  |  {counts}") }
            ));
            if items.is_empty() {
                sections.push("No data found".into());
            } else {
                sections.push(frames(items, command, verbose, p));
                let issues = issues(items, verbose, p);
                if !issues.is_empty() {
                    sections.push(issues);
                }
                if command == "discover" && !verbose && items.len() > 1 {
                    sections.push(
                        "Use --verbose for individual frames; --json for the complete report."
                            .into(),
                    );
                }
            }
        } else if command == "list" {
            if items.is_empty() && value["result"].is_object() {
                sections.push(fields(&value["result"], p));
            } else {
                sections.push(format!("Total: {}", items.len()));
                if items.is_empty() {
                    sections.push("No data found".into());
                } else {
                    sections.push(list(items, p));
                }
            }
        } else if !items.is_empty() {
            for item in items {
                sections.push(fields(item, p));
            }
        }
        if command != "list" && !value["result"].is_null() {
            sections.push(fields(&value["result"], p));
        }
    } else {
        sections.push(fields_excluding(value, p, &["schema_version", "command"]));
    }
    if value["items"].is_array() && !value["error"].is_null() {
        sections.push(format!("{}\n{}", p.title("error:"), fields(&value["error"], p)));
    }
    if value["interrupted"] == true {
        sections.push(p.title("Operation interrupted; results may be incomplete."));
    }
    if verbose {
        sections
            .push("diagnostics:\n  runtime: rust\n  report_schema: 1\n  secrets: redacted".into());
    }
    // The command heading stays on its own line for predictable text output.
    let heading = sections.remove(0);
    format!(
        "{heading}\n{}",
        sections.into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n\n")
    )
}

fn field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .filter(|value| !value.is_null())
        .map(display_value)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "-".into())
}

fn list(items: &[Value], p: &Presentation) -> String {
    let keys: &[&str] = if items[0].get("availability").is_some() {
        &["id", "availability", "products", "description"]
    } else if items[0].get("variables").is_some() {
        &["id", "variables", "units", "default", "historical", "mutable"]
    } else if items[0].get("country").is_some() {
        &["id", "country", "name", "status", "conflict", "snapshot_date", "capability"]
    } else {
        &["id", "name", "latitude", "longitude", "product_ids"]
    };
    let mut headers = keys.to_vec();
    headers[0] = if items[0].get("availability").is_some() {
        "source"
    } else if items[0].get("variables").is_some() {
        "product"
    } else if items[0].get("country").is_some() {
        "station"
    } else {
        "station"
    };
    p.table(
        &headers,
        items
            .iter()
            .map(|item| keys.iter().map(|key| p.status(&field(item, key))).collect())
            .collect(),
    )
}

fn timestamp(text: &str) -> String {
    DateTime::parse_from_rfc3339(text)
        .map(|time| time.with_timezone(&Utc).format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|_| safe_label(text))
}

fn age(text: &str) -> String {
    let Ok(time) = DateTime::parse_from_rfc3339(text) else {
        return "-".into();
    };
    let seconds = (Utc::now() - time.with_timezone(&Utc)).num_seconds();
    if seconds < 0 {
        return "future".into();
    }
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        86400..31536000 => format!("{}d", seconds / 86400),
        _ => format!("{}y", seconds / 31536000),
    }
}

fn frames(items: &[Value], command: &str, verbose: bool, p: &Presentation) -> String {
    // Group by status too, so a failing station can never hide in a success row.
    let mut groups: BTreeMap<(String, String, String), Vec<&Value>> = BTreeMap::new();
    for item in items {
        groups
            .entry((field(item, "source"), field(item, "product"), field(item, "status")))
            .or_default()
            .push(item);
    }
    let detailed = verbose || items.len() == 1 || command == "download";
    let mut rows = Vec::new();
    for ((source, product, status), group) in groups {
        let batches = if detailed {
            group.iter().map(|item| vec![*item]).collect::<Vec<_>>()
        } else {
            vec![group]
        };
        for batch in batches {
            let times =
                batch.iter().filter_map(|item| item["valid_time"].as_str()).collect::<Vec<_>>();
            let mut times = times;
            times.sort_by_cached_key(|time| timestamp(time));
            let earliest = times.first().copied();
            let latest = times.last().copied();
            let time = match (earliest, latest) {
                (Some(first), Some(last)) if first != last => {
                    format!("{} to {}", timestamp(first), timestamp(last))
                }
                (Some(first), _) => timestamp(first),
                _ => "-".into(),
            };
            let station =
                if detailed { field(batch[0], "station") } else { batch.len().to_string() };
            let mut row = vec![
                Cell::new(&source),
                Cell::new(&product),
                Cell::new(station),
                p.status(&status),
                Cell::new(time),
                Cell::new(earliest.map(age).unwrap_or_else(|| "-".into())),
            ];
            if command == "download" {
                row.push(Cell::new(field(batch[0], "output_uri")));
            }
            rows.push(row);
        }
    }
    let mut headers = vec![
        "source",
        "product",
        if detailed { "station" } else { "items" },
        "status",
        "valid time (UTC)",
        "oldest age",
    ];
    if command == "download" {
        headers.push("output");
    }
    let mut rendered = p.table(&headers, rows);
    if verbose {
        let details = items
            .iter()
            .filter_map(|item| {
                let mut lines = Vec::new();
                for key in ["base_time", "logical_id", "capabilities"] {
                    let value = item.get(key).or_else(|| item["frame"].get(key));
                    if let Some(value) = value.filter(|value| !value.is_null()) {
                        lines.push(format!("  {key}: {}", display_value(value)));
                    }
                }
                (!lines.is_empty()).then(|| {
                    format!(
                        "{} / {} / {}\n{}",
                        field(item, "source"),
                        field(item, "product"),
                        field(item, "station"),
                        lines.join("\n")
                    )
                })
            })
            .collect::<Vec<_>>();
        if !details.is_empty() {
            rendered.push_str(&format!(
                "\n\n{}\n{}",
                p.title("Frame details"),
                details.join("\n\n")
            ));
        }
    }
    rendered
}

fn issues(items: &[Value], verbose: bool, p: &Presentation) -> String {
    let mut grouped: BTreeMap<(String, String), Vec<&Value>> = BTreeMap::new();
    for item in items.iter().filter(|item| item["error"].is_object()) {
        grouped
            .entry((field(item, "status"), display_value(&item["error"])))
            .or_default()
            .push(item);
    }
    if grouped.is_empty() {
        return String::new();
    }
    let mut rows = Vec::new();
    for ((status, _), group) in grouped {
        let mut targets: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
        for item in &group {
            let stations =
                targets.entry((field(item, "source"), field(item, "product"))).or_default();
            if let Some(station) = item["station"].as_str() {
                stations.insert(safe_label(station));
            }
        }
        let affected = targets
            .into_iter()
            .map(|((source, product), stations)| {
                let stations = stations.into_iter().collect::<Vec<_>>().join(", ");
                if stations.is_empty() {
                    format!("{source}/{product}")
                } else {
                    format!("{source}/{product}: {stations}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let error = &group[0]["error"];
        let message = if verbose {
            format!(
                "{} (code: {}; stage: {}; retryable: {})",
                field(error, "message"),
                field(error, "code"),
                field(error, "stage"),
                field(error, "retryable")
            )
        } else {
            field(error, "message")
        };
        rows.push(vec![p.status(&status), Cell::new(affected), Cell::new(message)]);
    }
    format!("{}\n{}", p.title("Issues"), p.table(&["status", "affected targets", "reason"], rows))
}

fn fields(value: &Value, p: &Presentation) -> String {
    fields_excluding(value, p, &[])
}

fn fields_excluding(value: &Value, p: &Presentation, excluded: &[&str]) -> String {
    let Some(object) = value.as_object() else {
        return display_value(value);
    };
    let mut sections = Vec::new();
    let mut rows = Vec::new();
    for (key, value) in object {
        if excluded.contains(&key.as_str()) || value.is_null() {
            continue;
        }
        if value.is_object() {
            if !value.as_object().is_some_and(|object| object.is_empty()) {
                sections.push(format!("{}\n{}", p.title(&format!("{key}:")), fields(value, p)));
            }
        } else {
            rows.push(vec![
                Cell::new(format!("{}:", safe_label(key))),
                p.status(&display_value(value)),
            ]);
        }
    }
    if !rows.is_empty() {
        sections.insert(0, p.table(&["field", "value"], rows));
    }
    sections.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plain(width: u16) -> Presentation {
        Presentation { width, color: false, unicode: false }
    }

    fn fixture() -> Value {
        let items = (0..59).map(|index| json!({
            "source": "au", "product": "composite", "station": format!("AU{index:02}"),
            "status": "success", "valid_time": "2026-09-30T05:51:00.000000Z",
            "frame": {"source": "au", "valid_time": "2026-09-30T05:51:00.000000Z"}
        })).chain([json!({
            "source": "au", "product": "composite", "station": "AU99", "status": "upstream_failed",
            "error": {"code": "transport", "message": "transport request failed", "stage": "discover", "retryable": true}
        }), json!({
            "source": "th", "product": "composite", "station": "cmp1", "status": "success",
            "valid_time": "2023-04-22T14:30:00Z"
        })]).collect::<Vec<_>>();
        json!({"command": "discover", "query": {"source": "all", "latest": true},
            "counts": {"total": 61, "success": 60, "upstream_failed": 1, "timeout": 0}, "items": items})
    }

    #[test]
    fn aggregation_keeps_failures_visible_and_collapses_successful_stations() {
        let report = fixture();
        let output = render_with(&report, None, false, &plain(120));
        assert!(output.contains("Items: 61"));
        assert!(output.contains("60 success"));
        assert!(!output.contains("0 timeout"));
        assert!(output.contains("59"));
        assert!(output.contains("AU99"));
        assert!(output.contains("upstream_failed"));
        assert!(!output.contains("AU00"));
        assert_eq!(output.matches("transport request failed").count(), 1);
        assert!(output.lines().count() < 30, "{output}");
        assert!(output.contains("2023-04-22"));
        assert!(output.contains("oldest age"));
        assert!(!output.contains('\x1b'));
    }

    #[test]
    fn verbose_reveals_every_station_without_repeating_frame_objects() {
        let report = fixture();
        let output = render_with(&report, None, true, &plain(120));
        for index in 0..59 {
            assert!(output.contains(&format!("AU{index:02}")));
        }
        assert!(output.contains("retryable: true"));
        assert!(output.contains("diagnostics:"));
        assert!(!output.contains("frame:"));
    }

    #[test]
    fn narrow_and_normal_tables_wrap_without_losing_controls_or_targets() {
        let rows = vec![vec![
            Cell::new("vn"),
            Cell::new("DHA, NHB, NHT, PHA, PLE, PLI, QNH, TKY, VIN, VTR"),
            Cell::new("multiple candidates; select a time"),
        ]];
        for width in [20, 40, 60, 80, 120] {
            let output = plain(width).table(&["source", "stations", "reason"], rows.clone());
            for line in output.lines() {
                assert!(
                    console::measure_text_width(line) <= width as usize,
                    "width {width}: {line}"
                );
            }
            for station in ["DHA", "NHB", "NHT", "VTR"] {
                assert!(output.contains(station), "{output}");
            }
        }
        let report = json!({"command": "list", "items": [{"id": "TH\u{1b}[2J", "availability": "public", "products": [], "description": "fixture"}]});
        let output = render_with(&report, None, false, &plain(80));
        assert!(output.contains("TH [2J"));
        assert!(!output.contains('\x1b'));
    }

    #[test]
    fn matching_errors_across_sources_share_one_reason_and_keep_all_targets() {
        let report = json!({"command": "discover", "items": [
            {"source": "nz", "product": "rain", "status": "upstream_failed", "error": {"message": "transport request failed"}},
            {"source": "pt", "product": "composite", "status": "upstream_failed", "error": {"message": "transport request failed"}}
        ]});
        let output = render_with(&report, None, false, &plain(120));
        assert_eq!(output.matches("transport request failed").count(), 1);
        assert!(output.contains("nz/rain"));
        assert!(output.contains("pt/composite"));
    }

    #[test]
    fn timestamps_are_displayed_in_utc_and_unknown_timestamps_are_preserved() {
        assert_eq!(timestamp("2026-09-30T13:51:00+08:00"), "2026-09-30 05:51:00");
        assert_eq!(timestamp("unknown"), "unknown");
        assert_eq!(age("unknown"), "-");
        assert_eq!(age("2999-01-01T00:00:00Z"), "future");
    }
}
