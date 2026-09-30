from importlib.util import find_spec

from radiust._bridge import native_source_catalog
from radiust.registry import registry, sources


def test_python_source_namespace_has_no_provider_implementation():
    assert find_spec("radiust.sources") is None
    assert not hasattr(registry, "get")
    assert not hasattr(registry, "register")


def test_python_metadata_facade_reads_the_native_rust_catalog():
    native = native_source_catalog()
    native_ids = {item["id"] for item in native["sources"]}
    facade = {item.id: item for item in sources()}

    assert set(facade) == native_ids
    assert len(facade) == 24
    assert facade["uk"].availability == "retired"
    assert facade["tw"].default_product.id == "observation"
    assert facade["sg"].products[0].native_grid_kind == "cartesian"
