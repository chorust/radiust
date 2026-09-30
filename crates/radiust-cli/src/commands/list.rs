use crate::report;
use radiust_core::source::catalog::SourceCatalog;
use serde_json::{Value, json};

pub fn list(catalog: &SourceCatalog, kind: &str, source_id: Option<&str>) -> Result<Value, String> {
    if !matches!(kind, "sources" | "source" | "products" | "stations") {
        let source = catalog.source(kind).ok_or_else(|| format!("unknown source: {kind}"))?;
        let result = json!({
            "id": source.id,
            "description": source.description,
            "availability": source.availability,
            "products": source.products.iter().map(|product| product.id.clone()).collect::<Vec<_>>(),
            "stations": source.stations.iter().map(|station| station.id.clone()).collect::<Vec<_>>(),
        });
        let mut envelope = report::envelope("list", Vec::new(), Some(result));
        envelope["counts"] = json!({});
        return Ok(envelope);
    }

    let items: Vec<Value> = match kind {
        "sources" | "source" => catalog
            .sources
            .iter()
            .map(|source| {
                json!({
                    "id": source.id,
                    "description": source.description,
                    "availability": source.availability,
                    "products": source.products.iter().map(|product| product.id.clone()).collect::<Vec<_>>(),
                })
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
                        json!({
                            "id": product.id,
                            "variables": product.variables,
                            "units": product.units,
                            "historical": product.historical,
                            "default": product.default,
                            "mutable": product.mutable,
                        })
                    })
                    .collect(),
                _ => source
                    .stations
                    .iter()
                    .map(|station| {
                        json!({
                            "id": station.id,
                            "name": station.name,
                            "longitude": station.longitude,
                            "latitude": station.latitude,
                            "product_ids": station.product_ids,
                        })
                    })
                    .collect(),
            }
        }
        _ => unreachable!("source details were handled above"),
    };
    Ok(report::envelope("list", items, None))
}
