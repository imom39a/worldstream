# External input ingress example

The v1 gateway admits one deterministic, allowlisted input envelope. A host
pins the Room's exact Pack revision and basis Head; the durable preparation
row supplies the first recorded time if the request is retried.

```json
{
  "version": "worldstream/external-input-ingress.v1",
  "source_id": "01ARZ3NDEKTSV4RRFFQ69G5FH2",
  "input_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
  "input_type": "worldstream.external_input.v1",
  "based_on_room_seq": 7,
  "pack_digest": "blake3:0000000000000000000000000000000000000000000000000000000000000000",
  "payload": { "revision": 3, "value": "stable-example" }
}
```

```sh
curl -X POST "$WORLDSTREAM/v1/rooms/$ROOM_ID/external-input" \
  -H "Authorization: Bearer $HOST_CAPABILITY" \
  -H 'Content-Type: application/json' \
  --data-binary @external-input.json
```

The response contains the source/input identity, the durable recorded time,
the committed Transition identity, and the resulting Head. Repeating the same
identity and request returns the same receipt with `duplicate: true`; changing
the payload, Pack pin, or basis sequence is a conflict. The gateway never
rebases a stale request, and the Pack reducer remains responsible for the
meaning of revisions or corrections.
