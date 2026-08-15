from importlib.metadata import version

import worldstream_sdk


def test_package_is_explicitly_a_skeleton() -> None:
    assert worldstream_sdk.__version__ == "0.1.0"
    assert not hasattr(worldstream_sdk, "Client")


def test_frozen_dependency_surface_is_installed() -> None:
    assert version("pydantic") == "2.13.4"
    assert version("websockets") == "17.0.1"
