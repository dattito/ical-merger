{{- define "ical-merger.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "ical-merger.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name (include "ical-merger.name" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}

{{- define "ical-merger.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "ical-merger.labels" -}}
helm.sh/chart: {{ include "ical-merger.chart" . }}
app.kubernetes.io/name: {{ include "ical-merger.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end -}}

{{- define "ical-merger.selectorLabels" -}}
app.kubernetes.io/name: {{ include "ical-merger.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "ical-merger.config" -}}
listen = {{ .Values.config.listen | quote }}
default_timezone = {{ .Values.config.defaultTimezone | quote }}
horizon_days = {{ .Values.config.horizonDays }}
refresh_seconds = {{ .Values.config.refreshSeconds }}
output = {{ .Values.config.output | quote }}
{{- range .Values.config.sources }}

[[sources]]
url = {{ .url | quote }}
{{- with .timezone }}
timezone = {{ . | quote }}
{{- end }}
{{- end }}
{{- end -}}
