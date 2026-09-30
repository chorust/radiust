use radiust_core::legacy_display::apply_for_source;
use radiust_core::limits::Limits;
use radiust_core::preview::preview_bytes;

struct Golden {
    path_id: &'static str,
    source: &'static str,
    product: &'static str,
    station: Option<&'static str>,
    input: &'static [u8],
    baseline: &'static [u8],
}

const GOLDENS: &[Golden] = &[
    Golden {
        path_id: "au/composite",
        source: "au",
        product: "composite",
        station: None,
        input: include_bytes!("../../../tests/fixtures/legacy-display/au/source.png"),
        baseline: include_bytes!("../../../tests/fixtures/legacy-display/au/old-gray.png"),
    },
    Golden {
        path_id: "ca/rain",
        source: "ca",
        product: "rain",
        station: Some("CASFT"),
        input: include_bytes!("../../../tests/fixtures/legacy-display/ca/source.gif"),
        baseline: include_bytes!("../../../tests/fixtures/legacy-display/ca/old-gray.png"),
    },
    Golden {
        path_id: "es/composite",
        source: "es",
        product: "composite",
        station: Some("ESCOMP"),
        input: include_bytes!("../../../tests/fixtures/legacy-display/es/source.png"),
        baseline: include_bytes!("../../../tests/fixtures/legacy-display/es/old-gray.png"),
    },
    Golden {
        path_id: "fr/composite",
        source: "fr",
        product: "composite",
        station: Some("FRCOMP"),
        input: include_bytes!("../../../tests/fixtures/legacy-display/fr/source.png"),
        baseline: include_bytes!("../../../tests/fixtures/legacy-display/fr/old-gray.png"),
    },
    Golden {
        path_id: "id/composite",
        source: "id",
        product: "composite",
        station: None,
        input: include_bytes!("../../../tests/fixtures/legacy-display/id/source.png"),
        baseline: include_bytes!("../../../tests/fixtures/legacy-display/id/old-gray.png"),
    },
    Golden {
        path_id: "kr/composite",
        source: "kr",
        product: "composite",
        station: None,
        input: include_bytes!("../../../tests/fixtures/legacy-display/kr/source.png"),
        baseline: include_bytes!("../../../tests/fixtures/legacy-display/kr/old-gray.png"),
    },
    Golden {
        path_id: "my/composite/east",
        source: "my",
        product: "composite",
        station: None,
        input: include_bytes!("../../../tests/fixtures/legacy-display/my/east/source.png"),
        baseline: include_bytes!(
            "../../../tests/fixtures/legacy-display/my/east/archive-old-gray.png"
        ),
    },
    Golden {
        path_id: "my/composite/peninsular",
        source: "my",
        product: "composite",
        station: None,
        input: include_bytes!("../../../tests/fixtures/legacy-display/my/source.png"),
        baseline: include_bytes!("../../../tests/fixtures/legacy-display/my/old-gray.png"),
    },
    Golden {
        path_id: "nz/rain",
        source: "nz",
        product: "rain",
        station: Some("NZAU2"),
        input: include_bytes!("../../../tests/fixtures/legacy-display/nz/source.gif"),
        baseline: include_bytes!("../../../tests/fixtures/legacy-display/nz/old-gray.png"),
    },
    Golden {
        path_id: "pt/composite",
        source: "pt",
        product: "composite",
        station: Some("PTST2"),
        input: include_bytes!("../../../tests/fixtures/legacy-display/pt/source.png"),
        baseline: include_bytes!("../../../tests/fixtures/legacy-display/pt/old-gray.png"),
    },
    Golden {
        path_id: "sg/composite",
        source: "sg",
        product: "composite",
        station: None,
        input: include_bytes!("../../../tests/fixtures/legacy-display/sg/source.png"),
        baseline: include_bytes!("../../../tests/fixtures/legacy-display/sg/old-gray.png"),
    },
    Golden {
        path_id: "th/composite/kkn240Loop",
        source: "th",
        product: "composite",
        station: None,
        input: include_bytes!("../../../tests/fixtures/legacy-display/th/kkn240Loop/source.gif"),
        baseline: include_bytes!(
            "../../../tests/fixtures/legacy-display/th/kkn240Loop/old-gray.png"
        ),
    },
    Golden {
        path_id: "th_royalrain/cappi",
        source: "th_royalrain",
        product: "cappi",
        station: Some("takhli"),
        input: include_bytes!("../../../tests/fixtures/legacy-display/th_royalrain/source.png"),
        baseline: include_bytes!(
            "../../../tests/fixtures/legacy-display/th_royalrain/old-gray.png"
        ),
    },
    Golden {
        path_id: "tw/observation",
        source: "tw",
        product: "observation",
        station: None,
        input: include_bytes!("../../../tests/fixtures/legacy-display/tw/source.png"),
        baseline: include_bytes!("../../../tests/fixtures/legacy-display/tw/old-gray.png"),
    },
    Golden {
        path_id: "vn/cmax",
        source: "vn",
        product: "cmax",
        station: None,
        input: include_bytes!("../../../tests/fixtures/legacy-display/vn/source.png"),
        baseline: include_bytes!("../../../tests/fixtures/legacy-display/vn/old-gray.png"),
    },
];

#[test]
fn all_source_matched_legacy_display_goldens_match_pixel_for_pixel() {
    let limits =
        Limits { max_pixels: 100_000_000, max_temp_bytes: 800_000_000, ..Limits::default() };
    let mut mismatching_paths = Vec::new();
    for golden in GOLDENS {
        let source = preview_bytes(golden.input, &limits).unwrap();
        let expected = preview_bytes(golden.baseline, &limits).unwrap();
        let actual = apply_for_source(
            golden.source,
            golden.product,
            golden.station,
            &source.format,
            source.preview.width,
            source.preview.height,
            &source.preview.rgba,
            &limits,
        )
        .unwrap();

        assert!(actual.applied, "{} did not apply its reviewed rule", golden.path_id);
        assert_eq!(
            (actual.width, actual.height),
            (expected.preview.width, expected.preview.height),
            "{} output dimensions differ",
            golden.path_id
        );
        let differences = actual
            .rgba
            .chunks_exact(4)
            .zip(expected.preview.rgba.chunks_exact(4))
            .enumerate()
            .filter(|(_, (left, right))| left != right)
            .map(|(index, (left, right))| (index, left.to_vec(), right.to_vec()))
            .collect::<Vec<_>>();
        if !differences.is_empty() {
            let first = differences
                .iter()
                .take(12)
                .map(|(index, left, right)| {
                    ((index % actual.width as usize, index / actual.width as usize), left, right)
                })
                .collect::<Vec<_>>();
            let min_y = differences
                .iter()
                .map(|(index, _, _)| index / actual.width as usize)
                .min()
                .unwrap_or_default();
            let max_y = differences
                .iter()
                .map(|(index, _, _)| index / actual.width as usize)
                .max()
                .unwrap_or_default();
            eprintln!(
                "{} has {} pixel differences on rows {min_y}..={max_y}; first 12: {:?}",
                golden.path_id,
                differences.len(),
                first
            );
            mismatching_paths.push(golden.path_id);
        }
    }
    assert!(mismatching_paths.is_empty(), "pixel differences in {mismatching_paths:?}");
}

#[test]
fn blocked_legacy_display_paths_keep_original_pixels() {
    struct BlockedPath {
        path_id: &'static str,
        source: &'static str,
        product: &'static str,
        station: Option<&'static str>,
        format: &'static str,
        input: Option<&'static [u8]>,
    }

    const BLOCKED_PATHS: &[BlockedPath] = &[
        BlockedPath {
            path_id: "bmkg/composite",
            source: "bmkg",
            product: "composite",
            station: None,
            format: "PNG",
            input: None,
        },
        BlockedPath {
            path_id: "id_sidarma/cmax",
            source: "id_sidarma",
            product: "cmax",
            station: None,
            format: "PNG",
            input: None,
        },
        BlockedPath {
            path_id: "ph/composite",
            source: "ph",
            product: "composite",
            station: None,
            format: "PNG",
            input: None,
        },
        BlockedPath {
            path_id: "rainviewer/composite",
            source: "rainviewer",
            product: "composite",
            station: None,
            format: "PNG",
            input: Some(include_bytes!(
                "../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x0-y0.png"
            )),
        },
        BlockedPath {
            path_id: "th/composite/cmp1",
            source: "th",
            product: "composite",
            station: Some("cmp1"),
            format: "GIF",
            input: Some(include_bytes!("../../../tests/fixtures/sources/th/raw/cmp1.gif")),
        },
        BlockedPath {
            path_id: "tw-http/observation",
            source: "tw-http",
            product: "observation",
            station: None,
            format: "PNG",
            input: Some(include_bytes!("../../../tests/fixtures/sources/tw-http/raw/CV1_3600.png")),
        },
        BlockedPath {
            path_id: "tw/grid",
            source: "tw",
            product: "grid",
            station: None,
            format: "PNG",
            input: None,
        },
        BlockedPath {
            path_id: "windy/reflectivity",
            source: "windy",
            product: "reflectivity",
            station: None,
            format: "PNG",
            input: Some(include_bytes!(
                "../../../tests/fixtures/sources/windy/raw/tile-z1-x0-y0.png"
            )),
        },
    ];

    let limits = Limits::default();
    let original = [11, 22, 33, 255, 44, 55, 66, 127, 77, 88, 99, 0, 111, 122, 133, 255];
    for path in BLOCKED_PATHS {
        let decoded = path.input.map(|bytes| preview_bytes(bytes, &limits).unwrap());
        let (format, width, height, rgba) =
            decoded.as_ref().map_or((path.format, 2, 2, original.as_slice()), |source| {
                (
                    source.format.as_str(),
                    source.preview.width,
                    source.preview.height,
                    source.preview.rgba.as_slice(),
                )
            });
        let preview = apply_for_source(
            path.source,
            path.product,
            path.station,
            format,
            width,
            height,
            rgba,
            &limits,
        )
        .unwrap();

        assert!(!preview.applied, "{} unexpectedly applied a legacy rule", path.path_id);
        assert_eq!(
            (preview.width, preview.height),
            (width, height),
            "{} resized raw pixels",
            path.path_id
        );
        assert_eq!(preview.rgba, rgba, "{} changed unverified raw pixels", path.path_id);
        assert!(preview.rule_version.is_none(), "{} reported an unverified rule", path.path_id);
        assert!(preview.reason.is_some(), "{} omitted the blocked reason", path.path_id);
    }
}
