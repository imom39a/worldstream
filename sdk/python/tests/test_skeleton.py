from importlib.metadata import version

import worldstream_sdk


def test_package_exposes_public_room_client() -> None:
    assert worldstream_sdk.__version__ == "0.1.0"
    assert hasattr(worldstream_sdk, "Client")


def test_package_exposes_manifest_derived_client_contract_identity() -> None:
    identity = worldstream_sdk.CLIENT_CONTRACT_IDENTITY
    assert identity["schema"] == "worldstream/client-contract-identity/v1"
    assert identity["wire"] == "0.1"
    assert identity["config"] == 1
    assert identity["core_schema_version"] == "worldstream.core-room-state.v1"
    assert identity["hash_suite"] == "blake3-canonical-json-v1"
    assert len(identity["pack_executors"]) == 4
    assert any(
        row["pack_id"] == "worldstream.agent-heist"
        and row["explanatory_version"] == "0.1.0"
        and row["revision_digest"]
        == "blake3:b05a682f0923001914a800072ee68348e68b979453c93e033cba7916b99a4407"
        and row["selectable_for_new_rooms"] is True
        and row["runnable_for_retained_rooms"] is True
        for row in identity["pack_executors"]
    )


def test_frozen_dependency_surface_is_installed() -> None:
    assert version("pydantic") == "2.13.4"
    assert version("websockets") == "17.0.1"
