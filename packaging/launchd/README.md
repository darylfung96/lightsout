# Launchd: video-prune

`com.everydaylaws.video-prune.plist` runs the safe render-prune job daily at
23:30, before the midnight video build. It sets `PRUNE_APPLY=1`; a manual run
without that variable writes a plan and deletes nothing.

Install it on this machine:

```sh
cp packaging/launchd/com.everydaylaws.video-prune.plist ~/Library/LaunchAgents/
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.everydaylaws.video-prune.plist
launchctl print gui/$(id -u)/com.everydaylaws.video-prune
```

Because a recent episode may need a re-render, the job keeps the last three
episodes fully intact. It removes only
`episodes/<NNN>/comp/*/renders/` for older episodes whose master exists and
whose `pipeline/publish/queue.json` entry is `posted`; media, audio, comp
assets, snapshots, the outbox and the queue are never touched.

Inspect a run:

```sh
lightsout --workspace ~/programming/LightsOut status video-prune
cat ~/programming/LightsOut/jobs/video-prune/state/prune-$(date +%F).tsv
```

Unload without deleting the plist:

```sh
launchctl bootout gui/$(id -u)/com.everydaylaws.video-prune
```
