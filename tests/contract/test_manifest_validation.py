import json

import pytest
from radiust.errors import IntegrityError
from radiust.identity import digest
from radiust.storage.manifest import load_manifest, new_manifest


def _manifest():
    processing_spec = {"format": "netcdf"}
    return new_manifest(
        logical_id="a" * 64,
        revision="b" * 64,
        output_id="c" * 64,
        processing_spec=processing_spec,
        processing_hash=digest(processing_spec),
        artifacts=[{"name": "frame.nc", "relative_uri": "frame.nc", "sha256": "e" * 64, "size_bytes": 1}],
        raw_complete=False,
    )


@pytest.mark.parametrize("field,value", [("logical_id", "short"), ("revision", "short"), ("output_id", "short"), ("processing_hash", "short"), ("created_at", "")])
def test_manifest_rejects_missing_or_malformed_identity_fields(tmp_path, field, value):
    path = tmp_path / "frame.manifest.json"
    document = _manifest().as_dict()
    document[field] = value
    path.write_text(json.dumps(document), encoding="utf-8")

    with pytest.raises(IntegrityError):
        load_manifest(path)


def test_manifest_rejects_non_mapping_processing_spec(tmp_path):
    path = tmp_path / "frame.manifest.json"
    document = _manifest().as_dict()
    document["processing_spec"] = []
    path.write_text(json.dumps(document), encoding="utf-8")

    with pytest.raises(IntegrityError):
        load_manifest(path)
