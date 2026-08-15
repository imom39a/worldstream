import json
import re
from importlib.metadata import version
from pathlib import Path
from typing import Any

from blake3 import blake3

FIXTURE_PATH = Path(__file__).resolve().parents[3] / "tests/fixtures/core_v1_golden.json"
MAX_SAFE_INTEGER = 9_007_199_254_740_991
EMPTY_BLAKE3_HEX = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"

TOP_LEVEL_FIELDS = {
    "blake3_empty_known_hex",
    "genesis_record_bytes",
    "hash_vectors",
    "lifecycle_vectors",
    "prefix_complete_head_bytes",
    "schema",
    "transition_record_bytes",
}
LIFECYCLE_FIELDS = {
    "accepted_transitions",
    "archive_mandatory_reject",
    "durable_dispositions",
    "independent_transitions",
    "prefix_complete_head_bytes",
}
TRANSITION_VECTOR_FIELDS = {
    "complete_head_bytes",
    "digest",
    "hash_input_bytes",
    "name",
    "record_bytes",
}
ACCEPTED_TRANSITION_NAMES = (
    "atomic_role_swap",
    "enabled_to_suspended",
    "resume",
    "access_to_spectator",
    "access_to_participant",
    "depart",
    "rejoin_new_member_id",
    "archive_with_host_timer_cancellation",
    "post_archive_suspend",
)
INDEPENDENT_TRANSITION_NAMES = {
    "atomic_all_mandatory",
    "individual_role_change",
}
DURABLE_DISPOSITION_FIELDS = {
    "atomic_role_swap_rejection_bytes",
    "clean_rejection_existing_bytes",
    "clean_rejection_new_bytes",
    "no_change_existing_bytes",
    "no_change_new_bytes",
}
ADMINISTRATIVE_DISPOSITION_FIELDS = {
    "basis_complete_head",
    "disposition",
    "domain",
    "recorded_stimulus",
    "rejection",
}
REJECTION_FIELDS = {"bounded_safe_details", "declared_code"}
ARCHIVE_MANDATORY_REJECT_FIELDS = {
    "classification",
    "head_bytes_after",
    "head_bytes_before",
    "receipt_count",
    "transition_count",
}
VECTOR_FIELDS = {"canonical_bytes", "digest", "digest_bytes_hex"}
VECTOR_PREIMAGE_FIELDS = {
    "activity": {"activity", "domain", "pack_digest"},
    "authoritative": {
        "activity_state_hash",
        "core_schema",
        "core_state_hash",
        "domain",
        "pack_digest",
    },
    "core": {"core", "core_schema", "domain"},
    "genesis": {
        "codec_id",
        "configuration",
        "core_schema",
        "created_at",
        "domain",
        "hash_suite",
        "initial_activity_state_hash",
        "initial_authoritative_state_hash",
        "initial_core_state_hash",
        "initial_timers",
        "pack_digest",
        "room_id",
        "room_seed",
    },
    "transition": {
        "codec_id",
        "core_schema",
        "domain",
        "hash_suite",
        "ordered_attention_signals",
        "ordered_domain_events",
        "ordered_timer_changes",
        "pack_digest",
        "previous_transition_or_genesis_hash",
        "recorded_stimulus",
        "resulting_activity_state_hash",
        "resulting_authoritative_state_hash",
        "resulting_core_state_hash",
        "room_id",
        "room_seq",
    },
}
VECTOR_DOMAINS = {
    "activity": "worldstream/activity-state/v1",
    "authoritative": "worldstream/authoritative-state/v1",
    "core": "worldstream/core-state/v1",
    "genesis": "worldstream/genesis/v1",
    "transition": "worldstream/transition/v1",
}
GENESIS_RECORD_FIELDS = {
    "codec_id",
    "configuration",
    "core_schema_version",
    "created_at",
    "genesis_hash",
    "genesis_version",
    "hash_suite",
    "initial_activity_state",
    "initial_activity_state_hash",
    "initial_authoritative_state_hash",
    "initial_core_state",
    "initial_core_state_hash",
    "initial_timers",
    "pack_digest",
    "room_id",
    "room_seed",
}
TRANSITION_RECORD_FIELDS = {
    "codec_id",
    "core_schema_version",
    "hash_suite",
    "ordered_attention_signals",
    "ordered_domain_events",
    "ordered_timer_changes",
    "pack_digest",
    "previous_transition_or_genesis_hash",
    "recorded_stimulus",
    "resulting_activity_state",
    "resulting_activity_state_hash",
    "resulting_authoritative_state_hash",
    "resulting_core_state",
    "resulting_core_state_hash",
    "room_id",
    "room_seq",
    "transition_hash",
    "transition_version",
}
COMPLETE_HEAD_FIELDS = {
    "activity_state_hash",
    "authoritative_state_hash",
    "core_schema_version",
    "core_state_hash",
    "genesis_or_transition_hash",
    "pack_digest",
    "room_id",
    "room_seq",
}


def _object_without_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON object key: {key!r}")
        result[key] = value
    return result


def _safe_integer(token: str) -> int:
    value = int(token)
    if not -MAX_SAFE_INTEGER <= value <= MAX_SAFE_INTEGER:
        raise ValueError(f"integer is outside the canonical safe range: {token}")
    return value


def _reject_number(token: str) -> None:
    raise ValueError(f"non-integer JSON number is not canonical: {token}")


def _strict_loads(raw: str) -> Any:
    return json.loads(
        raw,
        object_pairs_hook=_object_without_duplicates,
        parse_int=_safe_integer,
        parse_float=_reject_number,
        parse_constant=_reject_number,
    )


def _validate_canonical_value(value: Any) -> None:
    if value is None or isinstance(value, (bool, str)):
        return
    if type(value) is int:
        if not -MAX_SAFE_INTEGER <= value <= MAX_SAFE_INTEGER:
            raise ValueError(f"integer is outside the canonical safe range: {value}")
        return
    if isinstance(value, list):
        for item in value:
            _validate_canonical_value(item)
        return
    if isinstance(value, dict):
        for key, item in value.items():
            if not isinstance(key, str):
                raise TypeError(f"JSON object key is not a string: {key!r}")
            _validate_canonical_value(item)
        return
    raise TypeError(f"unsupported canonical JSON value: {value!r}")


def _canonical_bytes(value: Any) -> bytes:
    _validate_canonical_value(value)
    return json.dumps(
        value,
        allow_nan=False,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")


def _parse_exact_canonical_bytes(raw: Any, label: str) -> Any:
    assert isinstance(raw, str), f"{label} must be a JSON string"
    parsed = _strict_loads(raw)
    assert _canonical_bytes(parsed) == raw.encode("utf-8"), f"{label} is not canonical JSON"
    return parsed


def _assert_exact_fields(value: Any, expected: set[str], label: str) -> dict[str, Any]:
    assert isinstance(value, dict), f"{label} must be an object"
    actual = set(value)
    assert actual == expected, (
        f"{label} fields differ: missing={sorted(expected - actual)}, "
        f"unexpected={sorted(actual - expected)}"
    )
    return value


def _load_fixture() -> dict[str, Any]:
    fixture = _strict_loads(FIXTURE_PATH.read_text(encoding="utf-8"))
    return _assert_exact_fields(fixture, TOP_LEVEL_FIELDS, "fixture")


def _transition_hash_input_from_record(record: dict[str, Any]) -> dict[str, Any]:
    return {
        "codec_id": record["codec_id"],
        "core_schema": record["core_schema_version"],
        "domain": record["transition_version"],
        "hash_suite": record["hash_suite"],
        "ordered_attention_signals": record["ordered_attention_signals"],
        "ordered_domain_events": record["ordered_domain_events"],
        "ordered_timer_changes": record["ordered_timer_changes"],
        "pack_digest": record["pack_digest"],
        "previous_transition_or_genesis_hash": record["previous_transition_or_genesis_hash"],
        "recorded_stimulus": record["recorded_stimulus"],
        "resulting_activity_state_hash": record["resulting_activity_state_hash"],
        "resulting_authoritative_state_hash": record["resulting_authoritative_state_hash"],
        "resulting_core_state_hash": record["resulting_core_state_hash"],
        "room_id": record["room_id"],
        "room_seq": record["room_seq"],
    }


def _complete_head_from_genesis(genesis: dict[str, Any]) -> dict[str, Any]:
    return {
        "activity_state_hash": genesis["initial_activity_state_hash"],
        "authoritative_state_hash": genesis["initial_authoritative_state_hash"],
        "core_schema_version": genesis["core_schema_version"],
        "core_state_hash": genesis["initial_core_state_hash"],
        "genesis_or_transition_hash": genesis["genesis_hash"],
        "pack_digest": genesis["pack_digest"],
        "room_id": genesis["room_id"],
        "room_seq": 0,
    }


def _complete_head_from_transition(record: dict[str, Any]) -> dict[str, Any]:
    return {
        "activity_state_hash": record["resulting_activity_state_hash"],
        "authoritative_state_hash": record["resulting_authoritative_state_hash"],
        "core_schema_version": record["core_schema_version"],
        "core_state_hash": record["resulting_core_state_hash"],
        "genesis_or_transition_hash": record["transition_hash"],
        "pack_digest": record["pack_digest"],
        "room_id": record["room_id"],
        "room_seq": record["room_seq"],
    }


def _parse_transition_vector(
    value: Any, label: str, previous_head: dict[str, Any]
) -> tuple[dict[str, Any], dict[str, Any]]:
    vector = _assert_exact_fields(value, TRANSITION_VECTOR_FIELDS, label)
    assert isinstance(vector["name"], str), f"{label}.name must be a string"

    hash_input = _assert_exact_fields(
        _parse_exact_canonical_bytes(vector["hash_input_bytes"], f"{label}.hash_input_bytes"),
        VECTOR_PREIMAGE_FIELDS["transition"],
        f"{label} hash input",
    )
    record = _assert_exact_fields(
        _parse_exact_canonical_bytes(vector["record_bytes"], f"{label}.record_bytes"),
        TRANSITION_RECORD_FIELDS,
        f"{label} record",
    )
    head = _assert_exact_fields(
        _parse_exact_canonical_bytes(vector["complete_head_bytes"], f"{label}.complete_head_bytes"),
        COMPLETE_HEAD_FIELDS,
        f"{label} CompleteHead",
    )

    assert hash_input == _transition_hash_input_from_record(record)
    digest = f"blake3:{blake3(vector['hash_input_bytes'].encode('utf-8')).hexdigest()}"
    assert isinstance(vector["digest"], str), f"{label}.digest must be a string"
    assert re.fullmatch(r"blake3:[0-9a-f]{64}", vector["digest"])
    assert vector["digest"] == digest
    assert record["transition_hash"] == digest
    assert head == _complete_head_from_transition(record)

    previous_hash = previous_head["genesis_or_transition_hash"]
    assert hash_input["previous_transition_or_genesis_hash"] == previous_hash
    assert record["previous_transition_or_genesis_hash"] == previous_hash
    assert record["room_seq"] == previous_head["room_seq"] + 1
    assert record["room_id"] == previous_head["room_id"]
    assert record["pack_digest"] == previous_head["pack_digest"]
    assert record["core_schema_version"] == previous_head["core_schema_version"]
    return record, head


def test_core_v1_hash_vectors_are_canonical_and_independently_hashed() -> None:
    fixture = _load_fixture()
    assert fixture["schema"] == "worldstream/core-v1-golden-corpus/v1"
    assert version("blake3") == "1.0.9"
    assert fixture["blake3_empty_known_hex"] == EMPTY_BLAKE3_HEX
    assert blake3(b"").hexdigest() == EMPTY_BLAKE3_HEX

    vectors = _assert_exact_fields(
        fixture["hash_vectors"], set(VECTOR_PREIMAGE_FIELDS), "hash_vectors"
    )
    for name, expected_preimage_fields in VECTOR_PREIMAGE_FIELDS.items():
        vector = _assert_exact_fields(vectors[name], VECTOR_FIELDS, f"hash_vectors.{name}")
        preimage = _parse_exact_canonical_bytes(
            vector["canonical_bytes"], f"hash_vectors.{name}.canonical_bytes"
        )
        preimage = _assert_exact_fields(
            preimage, expected_preimage_fields, f"hash_vectors.{name} preimage"
        )
        assert preimage["domain"] == VECTOR_DOMAINS[name]

        digest_bytes_hex = vector["digest_bytes_hex"]
        assert isinstance(digest_bytes_hex, str)
        assert re.fullmatch(r"[0-9a-f]{64}", digest_bytes_hex)
        digest = blake3(vector["canonical_bytes"].encode("utf-8"))
        assert digest.digest().hex() == digest_bytes_hex
        assert digest.hexdigest() == digest_bytes_hex
        assert vector["digest"] == f"blake3:{digest_bytes_hex}"


def test_core_v1_records_and_prefix_heads_are_exact_and_linked() -> None:
    fixture = _load_fixture()
    vectors = fixture["hash_vectors"]
    preimages = {name: _strict_loads(vector["canonical_bytes"]) for name, vector in vectors.items()}

    genesis = _assert_exact_fields(
        _parse_exact_canonical_bytes(fixture["genesis_record_bytes"], "genesis_record_bytes"),
        GENESIS_RECORD_FIELDS,
        "genesis record",
    )
    transition = _assert_exact_fields(
        _parse_exact_canonical_bytes(fixture["transition_record_bytes"], "transition_record_bytes"),
        TRANSITION_RECORD_FIELDS,
        "transition record",
    )

    assert preimages["core"] == {
        "core": genesis["initial_core_state"],
        "core_schema": genesis["core_schema_version"],
        "domain": VECTOR_DOMAINS["core"],
    }
    assert preimages["activity"] == {
        "activity": genesis["initial_activity_state"],
        "domain": VECTOR_DOMAINS["activity"],
        "pack_digest": genesis["pack_digest"],
    }
    assert preimages["authoritative"] == {
        "activity_state_hash": genesis["initial_activity_state_hash"],
        "core_schema": genesis["core_schema_version"],
        "core_state_hash": genesis["initial_core_state_hash"],
        "domain": VECTOR_DOMAINS["authoritative"],
        "pack_digest": genesis["pack_digest"],
    }
    assert preimages["genesis"] == {
        "codec_id": genesis["codec_id"],
        "configuration": genesis["configuration"],
        "core_schema": genesis["core_schema_version"],
        "created_at": genesis["created_at"],
        "domain": genesis["genesis_version"],
        "hash_suite": genesis["hash_suite"],
        "initial_activity_state_hash": genesis["initial_activity_state_hash"],
        "initial_authoritative_state_hash": genesis["initial_authoritative_state_hash"],
        "initial_core_state_hash": genesis["initial_core_state_hash"],
        "initial_timers": genesis["initial_timers"],
        "pack_digest": genesis["pack_digest"],
        "room_id": genesis["room_id"],
        "room_seed": genesis["room_seed"],
    }
    assert genesis["genesis_hash"] == vectors["genesis"]["digest"]
    assert genesis["initial_core_state_hash"] == vectors["core"]["digest"]
    assert genesis["initial_activity_state_hash"] == vectors["activity"]["digest"]
    assert genesis["initial_authoritative_state_hash"] == vectors["authoritative"]["digest"]

    assert preimages["transition"] == {
        "codec_id": transition["codec_id"],
        "core_schema": transition["core_schema_version"],
        "domain": transition["transition_version"],
        "hash_suite": transition["hash_suite"],
        "ordered_attention_signals": transition["ordered_attention_signals"],
        "ordered_domain_events": transition["ordered_domain_events"],
        "ordered_timer_changes": transition["ordered_timer_changes"],
        "pack_digest": transition["pack_digest"],
        "previous_transition_or_genesis_hash": transition["previous_transition_or_genesis_hash"],
        "recorded_stimulus": transition["recorded_stimulus"],
        "resulting_activity_state_hash": transition["resulting_activity_state_hash"],
        "resulting_authoritative_state_hash": transition["resulting_authoritative_state_hash"],
        "resulting_core_state_hash": transition["resulting_core_state_hash"],
        "room_id": transition["room_id"],
        "room_seq": transition["room_seq"],
    }
    assert transition["transition_hash"] == vectors["transition"]["digest"]
    assert transition["previous_transition_or_genesis_hash"] == genesis["genesis_hash"]

    raw_heads = fixture["prefix_complete_head_bytes"]
    assert isinstance(raw_heads, list)
    assert len(raw_heads) == 2
    heads = [
        _assert_exact_fields(
            _parse_exact_canonical_bytes(raw, f"prefix_complete_head_bytes[{index}]"),
            COMPLETE_HEAD_FIELDS,
            f"prefix CompleteHead[{index}]",
        )
        for index, raw in enumerate(raw_heads)
    ]
    assert heads == [
        {
            "activity_state_hash": genesis["initial_activity_state_hash"],
            "authoritative_state_hash": genesis["initial_authoritative_state_hash"],
            "core_schema_version": genesis["core_schema_version"],
            "core_state_hash": genesis["initial_core_state_hash"],
            "genesis_or_transition_hash": genesis["genesis_hash"],
            "pack_digest": genesis["pack_digest"],
            "room_id": genesis["room_id"],
            "room_seq": 0,
        },
        {
            "activity_state_hash": transition["resulting_activity_state_hash"],
            "authoritative_state_hash": transition["resulting_authoritative_state_hash"],
            "core_schema_version": transition["core_schema_version"],
            "core_state_hash": transition["resulting_core_state_hash"],
            "genesis_or_transition_hash": transition["transition_hash"],
            "pack_digest": transition["pack_digest"],
            "room_id": transition["room_id"],
            "room_seq": transition["room_seq"],
        },
    ]


def test_core_v1_lifecycle_transitions_are_canonical_hashed_and_chained() -> None:
    fixture = _load_fixture()
    lifecycle = _assert_exact_fields(
        fixture["lifecycle_vectors"], LIFECYCLE_FIELDS, "lifecycle_vectors"
    )

    accepted = lifecycle["accepted_transitions"]
    assert isinstance(accepted, list)
    assert tuple(vector.get("name") for vector in accepted) == ACCEPTED_TRANSITION_NAMES

    raw_prefix_heads = lifecycle["prefix_complete_head_bytes"]
    assert isinstance(raw_prefix_heads, list)
    assert len(raw_prefix_heads) == 10
    prefix_heads = [
        _assert_exact_fields(
            _parse_exact_canonical_bytes(
                raw, f"lifecycle_vectors.prefix_complete_head_bytes[{index}]"
            ),
            COMPLETE_HEAD_FIELDS,
            f"lifecycle prefix CompleteHead[{index}]",
        )
        for index, raw in enumerate(raw_prefix_heads)
    ]
    assert [head["room_seq"] for head in prefix_heads] == list(range(10))

    genesis = _assert_exact_fields(
        _parse_exact_canonical_bytes(fixture["genesis_record_bytes"], "genesis_record_bytes"),
        GENESIS_RECORD_FIELDS,
        "genesis record",
    )
    genesis_head = _complete_head_from_genesis(genesis)
    assert prefix_heads[0] == genesis_head
    assert raw_prefix_heads[0].encode("utf-8") == _canonical_bytes(genesis_head)

    previous_head = genesis_head
    for room_seq, (expected_name, vector) in enumerate(
        zip(ACCEPTED_TRANSITION_NAMES, accepted, strict=True), start=1
    ):
        label = f"lifecycle_vectors.accepted_transitions[{room_seq - 1}]"
        assert vector["name"] == expected_name
        record, head = _parse_transition_vector(vector, label, previous_head)
        assert record["room_seq"] == room_seq
        assert vector["complete_head_bytes"] == raw_prefix_heads[room_seq]
        assert head == prefix_heads[room_seq]
        previous_head = head

    legacy_prefix = fixture["prefix_complete_head_bytes"]
    assert isinstance(legacy_prefix, list)
    assert raw_prefix_heads[:2] == legacy_prefix
    assert (
        accepted[0]["hash_input_bytes"] == fixture["hash_vectors"]["transition"]["canonical_bytes"]
    )
    assert accepted[0]["digest"] == fixture["hash_vectors"]["transition"]["digest"]
    assert accepted[0]["record_bytes"] == fixture["transition_record_bytes"]

    independent = _assert_exact_fields(
        lifecycle["independent_transitions"],
        INDEPENDENT_TRANSITION_NAMES,
        "lifecycle_vectors.independent_transitions",
    )
    for name in sorted(INDEPENDENT_TRANSITION_NAMES):
        vector = independent[name]
        assert vector["name"] == name
        record, _head = _parse_transition_vector(
            vector, f"lifecycle_vectors.independent_transitions.{name}", genesis_head
        )
        assert record["room_seq"] == 1


def test_core_v1_lifecycle_dispositions_are_durable_and_non_mutating() -> None:
    fixture = _load_fixture()
    lifecycle = _assert_exact_fields(
        fixture["lifecycle_vectors"], LIFECYCLE_FIELDS, "lifecycle_vectors"
    )
    durable = _assert_exact_fields(
        lifecycle["durable_dispositions"],
        DURABLE_DISPOSITION_FIELDS,
        "lifecycle_vectors.durable_dispositions",
    )

    parsed: dict[str, dict[str, Any]] = {}
    for name in sorted(DURABLE_DISPOSITION_FIELDS):
        disposition = _assert_exact_fields(
            _parse_exact_canonical_bytes(
                durable[name], f"lifecycle_vectors.durable_dispositions.{name}"
            ),
            ADMINISTRATIVE_DISPOSITION_FIELDS,
            f"durable disposition {name}",
        )
        assert disposition["domain"] == "worldstream/administrative-disposition/v1"
        _assert_exact_fields(
            disposition["basis_complete_head"],
            COMPLETE_HEAD_FIELDS,
            f"durable disposition {name} basis CompleteHead",
        )
        parsed[name] = disposition

    assert durable["no_change_new_bytes"].encode("utf-8") == durable[
        "no_change_existing_bytes"
    ].encode("utf-8")
    assert durable["clean_rejection_new_bytes"].encode("utf-8") == durable[
        "clean_rejection_existing_bytes"
    ].encode("utf-8")

    for name in ("no_change_new_bytes", "no_change_existing_bytes"):
        assert parsed[name]["disposition"] == "no_change"
        assert parsed[name]["rejection"] is None
    for name in ("clean_rejection_new_bytes", "clean_rejection_existing_bytes"):
        assert parsed[name]["disposition"] == "rejected"
        rejection = _assert_exact_fields(
            parsed[name]["rejection"], REJECTION_FIELDS, f"durable disposition {name} rejection"
        )
        assert rejection == {
            "bounded_safe_details": {"safe": "fixture"},
            "declared_code": "fixture_veto",
        }

    lifecycle_prefix = lifecycle["prefix_complete_head_bytes"]
    basis_at_room_seq_five = _parse_exact_canonical_bytes(
        lifecycle_prefix[5], "lifecycle prefix CompleteHead[5]"
    )
    for name in (
        "no_change_new_bytes",
        "no_change_existing_bytes",
        "clean_rejection_new_bytes",
        "clean_rejection_existing_bytes",
    ):
        assert parsed[name]["basis_complete_head"] == basis_at_room_seq_five

    atomic_rejection = parsed["atomic_role_swap_rejection_bytes"]
    assert atomic_rejection["disposition"] == "rejected"
    rejection = _assert_exact_fields(
        atomic_rejection["rejection"], REJECTION_FIELDS, "atomic role-swap rejection"
    )
    assert rejection == {
        "bounded_safe_details": {"safe": "fixture"},
        "declared_code": "fixture_veto",
    }
    atomic_changeset = atomic_rejection["recorded_stimulus"]["canonical_changeset"]
    _assert_exact_fields(
        atomic_changeset,
        {"membership_changes", "room_status_change"},
        "atomic role-swap rejected changeset",
    )
    assert atomic_changeset["room_status_change"] is None
    assert [change["kind"] for change in atomic_changeset["membership_changes"]] == [
        "role_change",
        "role_change",
    ]
    assert len({change["member_id"] for change in atomic_changeset["membership_changes"]}) == 2
    accepted_atomic_record = _parse_exact_canonical_bytes(
        lifecycle["accepted_transitions"][0]["record_bytes"], "accepted atomic role-swap record"
    )
    assert atomic_changeset == accepted_atomic_record["recorded_stimulus"]["canonical_changeset"]

    genesis_head = _assert_exact_fields(
        _parse_exact_canonical_bytes(lifecycle_prefix[0], "lifecycle prefix CompleteHead[0]"),
        COMPLETE_HEAD_FIELDS,
        "genesis CompleteHead",
    )
    assert atomic_rejection["basis_complete_head"] == genesis_head

    archive_reject = _assert_exact_fields(
        lifecycle["archive_mandatory_reject"],
        ARCHIVE_MANDATORY_REJECT_FIELDS,
        "lifecycle_vectors.archive_mandatory_reject",
    )
    assert archive_reject["classification"] == "mandatory_stimulus_rejected_pack_fault"
    assert type(archive_reject["transition_count"]) is int
    assert type(archive_reject["receipt_count"]) is int
    assert archive_reject["transition_count"] == 0
    assert archive_reject["receipt_count"] == 0
    assert archive_reject["head_bytes_after"].encode("utf-8") == archive_reject[
        "head_bytes_before"
    ].encode("utf-8")
    head_before = _assert_exact_fields(
        _parse_exact_canonical_bytes(
            archive_reject["head_bytes_before"], "archive mandatory Reject Head before"
        ),
        COMPLETE_HEAD_FIELDS,
        "archive mandatory Reject Head before",
    )
    head_after = _assert_exact_fields(
        _parse_exact_canonical_bytes(
            archive_reject["head_bytes_after"], "archive mandatory Reject Head after"
        ),
        COMPLETE_HEAD_FIELDS,
        "archive mandatory Reject Head after",
    )
    assert head_before == head_after == genesis_head
    assert archive_reject["head_bytes_before"] == lifecycle_prefix[0]
