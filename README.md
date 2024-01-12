# ical-merger

## Introduction

The purpose of `ical-merger` is to take a list of calendar urls (in the `ical`-format) and return a single, merged calendar, accessible via the HTTP endpoint.

In the future, it should be able to be configurable, to e.g. hide the details of the events.

## Usage

To use it, there is a pre-built Docker image at `ghcr.io/dattito/ical-merger`.

It is configurable via these environment variables:

- `URLS`: A comma seperated list of the urls where the `ìcal`-calendars can be found (**REQUIRED**)
- `PORT`: The port on which the server is listening (default: `3000`)
- `HOST`: The host on which the server is listening (default: `0.0.0.0`)
- `HIDE_DETAILS`: Only start, end, uid and status of the events get published (default: `true`)
- `TZ_OFFSETS`: A comma seperated list of timezone offsets for the calendars. A list of integers, representing the hours. If the length is smaller then the lengh of the `URLS`, then the last value of the array ist used for the `URLS` at the end of the list (default: \[0\])
