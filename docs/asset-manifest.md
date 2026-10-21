# The asset manifest gate

A file existing is not evidence that it is the right file. `exists` proves a
shot was downloaded; it cannot tell a staff badge from a cow, because relevance
is a judgment and a gate can only read one. So the judgment is recorded in a
file, one line per shot, and the gate checks the record and the artifact
together. Before this gate, stage 2 of `spec/examples/video.md` passed on
`exists` per shot, so a fifteen-shot batch could include a cow standing in for a
staff badge and stage 3 would still begin.

## The manifest

`jobs/<name>/out/shots.json` — one JSON list, one object per storyboard shot:

```json
[
  {"shot": 1, "query": "staff badge", "file": "/abs/path/out/footage/01.mp4", "approved": true},
  {"shot": 2, "query": "lobby door",  "file": "/abs/path/out/footage/02.mp4", "approved": false}
]
```

| field | meaning |
|---|---|
| `shot` | the shot number the storyboard and the failure message use |
| `query` | the search or generation prompt that produced the asset, so a human knows what it was supposed to be |
| `file` | the asset on disk: an absolute path, or one relative to the directory the gate runs in |
| `approved` | literal JSON `true` once a human has looked at the shot; absent, `false`, `"yes"` and `1` all fail |

## The gate

```sh
bin/lightsout gate manifest --file out/shots.json --min 15
```

It exits `1` and names the first shot a human has to act on:

```
FAIL manifest: shot 2 'lobby door' is not approved
FAIL manifest: shot 4 'staff badge': /abs/path/out/footage/04.mp4 is missing or empty
FAIL manifest: 12 shots, want at least 15
```

The video storyboard asks for fifteen shots, so stage 2 calls the gate with
`--min 15`. A manifest that lists twelve shots fails the count even when all
twelve carry `approved: true`, because the episode it feeds still needs the
other three. When the count passes, the gate walks the entries and stops at the
first one whose `approved` flag is not `true`, which gives the message a shot
number and a query a reviewer can act on without opening the JSON. Only after
every entry is approved does the gate open each `file`, because a download that
failed is repaired by re-acquiring it, while a picture that showed the wrong
thing is rejected by a person; the two problems need different actions, so they
are reported in that order.

`json.load` failing, a top-level object instead of a list, or a list with no
entries all end the same way: the gate reports the problem and exits `1`. The
empty list is worth its own sentence, because acquisition can write one on a
quiet failure, and the gate then fails the stage since none of the fifteen
storyboard shots exist to approve.

## How a job calls it

```sh
u_say "stage 2: acquire footage"
# ...write $OUT/shots.json, with approved:false on every shot it acquires...
U_BLOCK_CLASS=judgment u_gate manifest --file "$OUT/shots.json" --min 15
```

`u_gate` turns a failed gate into a block: exit `2`, reason
`gate failed: manifest --file ...`, visible in `bin/job status` and
`bin/job diagnose`. The class is `judgment` rather than the default, because the
marker says a human must edit the artifact and re-run the gate, and the artifact
here is the manifest. The acquiring stage writes `false` on every shot it
fetches, and a reviewer's edit is the only thing that makes a flag `true`;
a job that flipped its own flags would be approving its own work, which is the
check the stage exists to provide. Approval is an edit to the manifest, then a
re-run; the gate still decides.

## Practical notes

- A rebuild that re-acquires shots must carry each unchanged shot's `file` and
  `approved` into the new manifest. Approvals live only in this file, and
  `jobs/*/out/` is gitignored, so a rewrite that starts from `approved: false`
  sends every shot back to the reviewer.
- This gate judges provenance, not encoding. Pair it with `duration`,
  `resolution` and `loudness` on the master; those decide the cut, this decides
  the picture.
- The block belongs to the `judgment` class, because the fix is an edited
  artifact and a re-run: the job passes `U_BLOCK_CLASS=judgment`, and no
  allowlisted repair can approve a shot.
