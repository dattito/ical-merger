# ical-merger

## Introduction

The purpose of `ical-merger` is to take a list of calendar urls (in the `ìcal`-format) and return a single, merged calendar, accessible via the HTTP endpoint.

In the future, it should be able to be configurable, to e.g. hide the details of the events.

## Usage

To use it, there is a pre-built Docker image at `ghcr.io/dattito/ical-merger`.

It is configurable via these environment variables:

- `URLS`: A comma seperated list of the urls where the `ìcal`-calendars can be found (**REQUIRED**)
- `PORT`: The port on which the server is listening (default: `3000`)
- `HOST`: The host on which the server is listening (default: `0.0.0.0`)
