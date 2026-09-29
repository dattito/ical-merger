# ical-merger

`ical-merger` reads calendar subscription URLs and serves one combined iCalendar feed. By default it hides event details and publishes merged `Busy` blocks, so gaps in the feed show availability.

The server accepts HTTP and HTTPS calendar URLs. It understands VEVENT and VFREEBUSY components, recurring events, recurrence exceptions, IANA timezone IDs, common Outlook timezone names, and embedded VTIMEZONE rules. Times are resolved to UTC before calendars are combined.

## Configuration

Set exactly one of `ICAL_MERGER_CONFIG` (TOML text) or `ICAL_MERGER_CONFIG_FILE` (path to a TOML file). A default IANA timezone is required for floating times; source entries can override it.

```toml
listen = "0.0.0.0:3000"
default_timezone = "Europe/Berlin"
horizon_days = 90
refresh_seconds = 900
output = "busy" # busy or title

[[sources]]
url = "https://calendar.example.net/personal.ics"

[[sources]]
url = "https://calendar.example.net/work.ics"
timezone = "America/New_York"
```

The server rejects missing or ambiguous configuration, malformed TOML, an invalid listen socket, empty source lists, non-HTTP URLs, invalid IANA zones, and nonpositive horizon or refresh values. Validation errors name the configuration field and omit URL credentials. Defaults are `listen = "0.0.0.0:3000"`, `horizon_days = 90`, `refresh_seconds = 900`, and `output = "busy"`.

The feed window starts at midnight in `default_timezone` and ends at midnight after the configured number of days. Each successful source response is cached for `refresh_seconds`. Failed sources are omitted from that request; a successful response includes `X-Ical-Merger-Partial`, `X-Ical-Merger-Skipped-Sources`, and `X-Ical-Merger-Skipped-Events` headers. If no source is usable, the feed endpoint returns 503. Source URLs and credentials are not written to logs or copied to the output.

Busy mode merges overlapping and adjacent occupied intervals. Title mode keeps events separate and publishes only titles; events marked PRIVATE or CONFIDENTIAL still appear as `Busy`. Both modes omit cancelled, tentative, transparent, and provider-marked-free events. All-day DTEND values are exclusive. The server caps each downloaded calendar at 10 MiB and bounds recurrence expansion; an event that cannot be interpreted or expanded safely is skipped and counted.

Period-valued RDATE entries and `RECURRENCE-ID;RANGE=THISANDFUTURE` series are skipped and counted. Embedded custom timezone transition rules are resolved for local dates from 1970 through 2050; an event outside that range is skipped rather than assigned a guessed offset.

## Run locally

With Nix installed:

```sh
nix develop
ICAL_MERGER_CONFIG_FILE=./config.toml cargo run --locked
```

The server exposes the calendar at `/` and a liveness check at `/healthz`. The calendar response uses `text/calendar; charset=utf-8`.

To run all checks in the pinned Nix environment:

```sh
nix flake check
```

To build and load the Docker-compatible image:

```sh
nix build .#dockerImage
docker load --input result
docker run --rm -p 3000:3000 \
  -e ICAL_MERGER_CONFIG_FILE=/etc/ical-merger/config.toml \
  -v "$PWD/config.toml:/etc/ical-merger/config.toml:ro" \
  ical-merger:latest
```

## Kubernetes

The example in [`deploy/kubernetes/ical-merger.yaml`](deploy/kubernetes/ical-merger.yaml) mounts TOML configuration from a ConfigMap. Changes to the mounted configuration require a pod restart. Put the configuration in a Kubernetes Secret if any subscription URL contains a credential or private token.

The Helm chart in [`charts/ical-merger`](charts/ical-merger) exposes source URLs, timezones, feed mode, horizon, refresh interval, and listener address in its values. Set `config.existingConfigSecret` to mount a complete TOML configuration from a Secret when the source URLs contain credentials. The chart's ConfigMap checksum triggers a rollout after values change.

The generated feed is public and has no built-in authentication. Restrict access at the ingress or network layer when needed. `output = "title"` applies to the whole server and reveals event titles except for private or confidential events.

## Development and releases

The Nix flake pins Nixpkgs, Crane, Cargo dependencies, development tools, and the Docker image build. `nix flake check` runs formatting, Clippy, tests, and a package build. GitHub Actions builds and smoke-tests native amd64 and arm64 images; version tags publish a multi-architecture image to `ghcr.io/dattito/ical-merger`.
