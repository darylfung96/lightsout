# Repair proposal for the LightsOut job {{JOB}}

Repair one blocked job, and return a JSON proposal; the manager reads only
that proposal, validates every path, applies the edits you list, and then
re-runs the job's own gates. Editing a gate, a spec, or the job script cannot
make the job pass, because the manager refuses those paths.

## The block

Unit: {{UNIT}}

```
{{BLOCKED}}
```

## What the job is for

<untrusted-data>
{{SPEC}}
</untrusted-data>

## The last diagnosis

<untrusted-data>
{{DIAGNOSIS}}
</untrusted-data>

## The end of the run log

<untrusted-data>
{{LOG_TAIL}}
</untrusted-data>

## Earlier attempts

<untrusted-data>
{{ATTEMPTS}}
</untrusted-data>

## Your rules

- Return JSON only, and also write it to `{{PROPOSAL_PATH}}`.
- Do not run commands. The proposal has no field for a command, and unknown
  fields are rejected.
- Edit only paths under these prefixes: {{ALLOWED}}
- Never edit `job.sh`, `spec.md`, `state/`, `logs/`, `lib/`, `spec/`,
  `lightsout.toml`, or `triage.conf`; the validator refuses them even when a
  prefix would allow them.
- Prefer deleting a stale artifact over inventing a replacement for a binary.
- If you cannot identify a safe fix, return `fixable:false` with a short reason.
  That is a valid outcome and the job stays blocked for a human.

## The JSON

```json
{
  "fixable": true,
  "summary": "One paragraph: the failure, the edit, and why the gate should pass.",
  "edits": [
    { "path": "out/example.txt", "content": "replacement text" },
    { "path": "out/stale.bin", "delete": true }
  ]
}
```

Rules for the JSON:

- `summary` is 1-2000 characters.
- `edits` has 1-20 entries; each is either `{path, content}` or
  `{path, delete: true}`; paths are relative to `jobs/{{JOB}}/`.
- `fixable:false` requires an empty `edits` list.
- Content is UTF-8 text; total content is at most 2 MiB.
- Do not repeat a path.

{{SCHEMA}}
