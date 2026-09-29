# ical-merger Helm chart

The chart deploys the public iCalendar feed server and creates its TOML
configuration from Helm values.

```sh
helm upgrade --install availability ./charts/ical-merger \
  --set config.defaultTimezone=Europe/Berlin \
  --set-string 'config.sources[0].url=https://calendar.example.net/personal.ics'
```

For multiple sources, per-source timezone overrides, or ingress settings, use
a values file:

```yaml
config:
  listen: "0.0.0.0:3000"
  defaultTimezone: Europe/Berlin
  horizonDays: 90
  refreshSeconds: 900
  output: busy
  sources:
    - url: https://calendar.example.net/personal.ics
    - url: https://calendar.example.net/work.ics
      timezone: America/New_York

ingress:
  enabled: true
  className: nginx
  hosts:
    - host: availability.example.net
      paths:
        - path: /
          pathType: Prefix
```

All server settings are available below `config` in `values.yaml`. The ConfigMap
checksum triggers a Deployment rollout when those values change. For
credential-bearing URLs, create a Kubernetes Secret with a `config.toml` key
containing the complete server configuration, then set
`config.existingConfigSecret` to its name. Changes to an external Secret
require a pod restart.

The feed is served at `/` and is public by default. Protect it at the ingress
or network layer if required.

The manifest smoke-test values used in CI are in `ci/values.yaml`.
