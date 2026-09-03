# Release download metrics

GitHub exposes cumulative counters for release assets, not unique users or the time of each
download. The scheduled `download-metrics.yml` workflow therefore records one UTC snapshot
per day so changes can be calculated from that point forward.

The first snapshot is versioned with the implementation. Later snapshots are committed by
`github-actions[bot]` to the dedicated `metrics/downloads` branch, keeping generated daily
data off `main`.

Each JSON file contains all public releases and assets plus convenience totals for Sagascript
distribution files. Draft releases are excluded; public prereleases are included.
