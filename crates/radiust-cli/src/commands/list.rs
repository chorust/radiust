use crate::report;
use radiust_core::source::catalog::SourceCatalog;
use serde_json::{Value, json};

pub fn list(catalog: &SourceCatalog, kind: &str, source_id: Option<&str>) -> Result<Value, String> {
    if !matches!(kind, "sources" | "source" | "products" | "stations") {
        let source = catalog.source(kind).ok_or_else(|| format!("unknown source: {kind}"))?;
        let mut result = json!({
            "id": source.id,
            "description": source.description,
            "availability": source.availability,
            "products": source.products.iter().map(|product| product.id.clone()).collect::<Vec<_>>(),
            "stations": source.stations.iter().map(|station| station.id.clone()).collect::<Vec<_>>(),
        });
        if let Some(metadata) = source.metadata.as_ref() {
            result["metadata"] = json!(metadata);
        }
        let mut envelope = report::envelope("list", Vec::new(), Some(result));
        envelope["counts"] = json!({});
        return Ok(envelope);
    }

    let items: Vec<Value> = match kind {
        "sources" | "source" => catalog
            .sources
            .iter()
            .map(|source| {
                let mut item = json!({
                    "id": source.id,
                    "description": source.description,
                    "availability": source.availability,
                    "products": source.products.iter().map(|product| product.id.clone()).collect::<Vec<_>>(),
                });
                if let Some(metadata) = source.metadata.as_ref() {
                    item["metadata"] = json!(metadata);
                }
                item
            })
            .collect(),
        "products" | "stations" => {
            let source_id = source_id.ok_or_else(|| format!("list {kind} requires a source id"))?;
            let source = catalog
                .source(source_id)
                .ok_or_else(|| format!("unknown source: {source_id}"))?;
            match kind {
                "products" => source
                    .products
                    .iter()
                    .map(|product| {
                        let mut item = json!({
                            "id": product.id,
                            "variables": product.variables,
                            "units": product.units,
                            "historical": product.historical,
                            "default": product.default,
                            "mutable": product.mutable,
                        });
                        if let Some(metadata) = product.metadata.as_ref() {
                            item["metadata"] = json!(metadata);
                        }
                        item
                    })
                    .collect(),
                _ => source
                    .stations
                    .iter()
                    .map(|station| {
                        let mut item = json!({
                            "id": station.id,
                            "name": station.name,
                            "longitude": station.longitude,
                            "latitude": station.latitude,
                            "product_ids": station.product_ids,
                        });
                        if source.id == "rdcap"
                            && let Some(metadata) = station.metadata.as_ref()
                        {
                            item["country"] = metadata
                                .country
                                .as_ref()
                                .map_or(Value::Null, |country| json!(country));
                            item["country_name"] = metadata
                                .extensions
                                .get("country_name")
                                .cloned()
                                .unwrap_or(Value::Null);
                            item["directory_statuses"] = metadata
                                .extensions
                                .get("directory_statuses")
                                .cloned()
                                .unwrap_or(Value::Null);
                            item["status"] = item["directory_statuses"].clone();
                            item["directory_conflicts"] = json!(metadata.directory_conflicts);
                            item["conflict"] = json!(metadata
                                .directory_conflicts
                                .iter()
                                .map(|conflict| {
                                    let values = conflict
                                        .values
                                        .iter()
                                        .map(|value| match value {
                                            Value::String(value) => value.clone(),
                                            _ => value.to_string(),
                                        })
                                        .collect::<Vec<_>>()
                                        .join("/");
                                    format!("{}: {values}", conflict.field)
                                })
                                .collect::<Vec<_>>()
                                .join(", "));
                            item["snapshot_date"] = metadata
                                .extensions
                                .get("snapshot_date")
                                .cloned()
                                .unwrap_or(Value::Null);
                            item["catalog_status"] = json!("offline_snapshot");
                            item["capabilities"] = metadata
                                .country
                                .as_ref()
                                .and_then(|country| {
                                    source.metadata.as_ref()?.country_capabilities.get(country)
                                })
                                .and_then(|capabilities| serde_json::to_value(capabilities).ok())
                                .unwrap_or(Value::Null);
                            item["capability"] = item["capabilities"].as_object().map_or(
                                Value::Null,
                                |capabilities| {
                                    let short = |name: &str, label: &str| {
                                        capabilities
                                            .get(name)
                                            .and_then(Value::as_str)
                                            .map(|status| format!("{label}:{status}"))
                                            .unwrap_or_else(|| format!("{label}:unknown"))
                                    };
                                    json!([
                                        short("discovery", "disc"),
                                        short("raw_acquisition", "raw"),
                                        short("science", "science"),
                                        short("readback", "readback"),
                                    ].join(" "))
                                },
                            );
                            item["metadata"] = json!(metadata);
                        }
                        item
                    })
                    .collect(),
            }
        }
        _ => unreachable!("source details were handled above"),
    };
    Ok(report::envelope("list", items, None))
}
