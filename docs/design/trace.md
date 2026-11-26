# Stroke trace JSON

A trace describes one stroke. The top-level object has a `phases` array in
recording order. Each entry has a `phase` string and a `values` array. The
phase names are `delivery`, `impact`, `launch`, `flight`, `bounce`, `skid`,
`roll`, `cup`, `lie`, and `ruling`. Each bounce has its own entry. The first
`bounce` entry is bounce 1.

The `values` array keeps recording order and can contain repeated names.
Both arrays can be empty. Each value is an object with these fields.

| Field | Type | Meaning |
| --- | --- | --- |
| `name` | String | Name of the recorded quantity |
| `value` | Number | Finite 64-bit floating-point value |
| `unit` | String | Unit such as `m/s` or `rpm`. Use `1` for a dimensionless value. |
| `source` | Object | Where the number came from |

The `source` object has a `kind` string. Its `name` field identifies the
input for each kind with a named source.

| `kind` | `name` |
| --- | --- |
| `tunable` | Name of the tunable, such as `AERO_DRAG` |
| `turf` | Name of the turf value, such as `fairway_firmness` |
| `course` | Name of the course value, such as `cup_radius` |
| `seed_draw` | Name of the draw, such as `lie/3` |
| `computed` | Omitted. The number was computed by the simulation. |

Serialization rejects NaN and infinities. Finite values retain their bits
when read back with the workspace's `serde_json` configuration.

This example records one flight value and one value for each of two bounces.

```json
{
	"phases": [
		{"phase": "flight", "values": [
			{"name": "drag", "value": 0.25, "unit": "1",
				"source": {"kind": "tunable", "name": "AERO_DRAG"}}
		]},
		{"phase": "bounce", "values": [
			{"name": "firmness", "value": 0.75, "unit": "1",
				"source": {"kind": "turf", "name": "fairway_firmness"}}
		]},
		{"phase": "bounce", "values": [
			{"name": "speed_out", "value": 4.5, "unit": "m/s",
				"source": {"kind": "computed"}}
		]}
	]
}
```
