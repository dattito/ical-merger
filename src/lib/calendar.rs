use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, Offset,
    TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;
use icalendar::{Calendar, CalendarDateTime, Component, DatePerhapsTime, Event, EventLike};
use mailrs_ical::{RawComponent, RawProperty, VTimezone};
use rrule::{RRuleSet, Tz as RRuleTz};
use uuid::Uuid;

use crate::config::{OutputMode, SourceConfig};

const MAX_RRULE_WORK: i64 = 100_000;
const MAX_OCCURRENCES: usize = 60_000;
const MAX_EVENT_DURATION_DAYS: i64 = 366;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interval {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ParsedSource {
    events: Vec<CalendarEvent>,
    freebusy: Vec<Interval>,
    pub skipped_events: usize,
    pub total_events: usize,
}

#[derive(Debug, Clone)]
struct CalendarEvent {
    uid: String,
    start: Option<IcalDateTime>,
    end: Option<IcalDateTime>,
    duration: Option<CalendarDuration>,
    summary: Option<String>,
    status: Option<String>,
    transparency: Option<String>,
    class: Option<String>,
    microsoft_busy_status: Option<String>,
    rrule: Option<String>,
    rdates: Vec<IcalDateTime>,
    exdates: Vec<IcalDateTime>,
    recurrence_id: Option<IcalDateTime>,
    recurrence_range: Option<String>,
}

#[derive(Debug, Clone)]
struct IcalDateTime {
    value: DateValue,
    zone: ResolvedZone,
}

#[derive(Debug, Clone)]
enum DateValue {
    Date(NaiveDate),
    DateTime(NaiveDateTime),
    Utc(DateTime<Utc>),
}

#[derive(Debug, Clone)]
enum ResolvedZone {
    Iana(Tz),
    Embedded(mailrs_ical::vtimezone::ResolvedTz),
}

#[derive(Debug, Clone, Copy, Default)]
struct CalendarDuration {
    days: i64,
    seconds: i64,
}

#[derive(Debug, Clone)]
struct Occurrence {
    source_index: usize,
    uid: String,
    recurrence_key: i64,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    title: String,
    private: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CalendarError {
    #[error("calendar syntax is invalid")]
    InvalidCalendar,
    #[error("calendar has no usable components")]
    NoUsableComponents,
    #[error("calendar output could not be serialized")]
    Serialize,
}

impl ParsedSource {
    pub fn parse(
        input: &str,
        source: &SourceConfig,
        default_timezone: Tz,
    ) -> Result<Self, CalendarError> {
        let root = mailrs_ical::parse::parse_calendar(input)
            .map_err(|_| CalendarError::InvalidCalendar)?;
        let total_events = root
            .children
            .iter()
            .filter(|component| {
                component.name.eq_ignore_ascii_case("VEVENT")
                    || component.name.eq_ignore_ascii_case("VFREEBUSY")
            })
            .count();
        if total_events > MAX_OCCURRENCES {
            return Err(CalendarError::InvalidCalendar);
        }
        let timezones = collect_timezones(&root);
        let floating_timezone = source.timezone.unwrap_or(default_timezone);
        let mut events = Vec::new();
        let mut freebusy = Vec::new();
        let mut skipped_events = 0;
        let mut parsed_event_count = 0;

        for (component_index, component) in root.children.iter().enumerate() {
            if component.name.eq_ignore_ascii_case("VEVENT") {
                parsed_event_count += 1;
                match parse_event(component, component_index, &timezones, floating_timezone) {
                    Ok(event) => events.push(event),
                    Err(()) => skipped_events += 1,
                }
            } else if component.name.eq_ignore_ascii_case("VFREEBUSY") {
                parsed_event_count += 1;
                match parse_freebusy(component, &timezones, floating_timezone) {
                    Ok(mut periods) => freebusy.append(&mut periods),
                    Err(()) => skipped_events += 1,
                }
            }
        }

        if parsed_event_count > 0 && events.is_empty() && freebusy.is_empty() && skipped_events > 0
        {
            return Err(CalendarError::NoUsableComponents);
        }

        Ok(Self {
            events,
            freebusy,
            skipped_events,
            total_events: parsed_event_count,
        })
    }
}

fn collect_timezones(root: &RawComponent) -> Vec<VTimezone> {
    root.children
        .iter()
        .filter(|component| component.name.eq_ignore_ascii_case("VTIMEZONE"))
        .filter_map(|component| {
            let tzid = property(component, "TZID")?;
            Some(VTimezone {
                tzid: tzid.value.trim().trim_matches('"').into(),
                raw_subs: component.children.clone(),
            })
        })
        .collect()
}

fn parse_event(
    component: &RawComponent,
    component_index: usize,
    timezones: &[VTimezone],
    floating_timezone: Tz,
) -> Result<CalendarEvent, ()> {
    let start = property(component, "DTSTART")
        .map(|property| parse_datetime(property, timezones, floating_timezone))
        .transpose()?;
    let recurrence_id = property(component, "RECURRENCE-ID")
        .map(|property| parse_datetime(property, timezones, floating_timezone))
        .transpose()?;
    if start.is_none() && recurrence_id.is_none() {
        return Err(());
    }

    let end = property(component, "DTEND")
        .map(|property| parse_datetime(property, timezones, floating_timezone))
        .transpose()?;
    let duration = property(component, "DURATION")
        .map(|property| parse_duration(&property.value))
        .transpose()?;
    if end.is_some() && duration.is_some() {
        return Err(());
    }

    let uid = property(component, "UID")
        .map(|property| property.value.trim().to_owned())
        .filter(|uid| !uid.is_empty())
        .unwrap_or_else(|| format!("missing-uid-{component_index}"));

    let rrules = properties(component, "RRULE");
    if rrules.len() > 1 {
        return Err(());
    }
    let rrule = rrules
        .first()
        .map(|property| property.value.trim().to_owned());
    let rdates = parse_datetime_list(component, "RDATE", timezones, floating_timezone)?;
    let exdates = parse_datetime_list(component, "EXDATE", timezones, floating_timezone)?;

    Ok(CalendarEvent {
        uid,
        start,
        end,
        duration,
        summary: property(component, "SUMMARY").map(|property| unescape_text(&property.value)),
        status: property(component, "STATUS")
            .map(|property| property.value.trim().to_ascii_uppercase()),
        transparency: property(component, "TRANSP")
            .map(|property| property.value.trim().to_ascii_uppercase()),
        class: property(component, "CLASS")
            .map(|property| property.value.trim().to_ascii_uppercase()),
        microsoft_busy_status: property(component, "X-MICROSOFT-CDO-BUSYSTATUS")
            .map(|property| property.value.trim().to_ascii_uppercase()),
        rrule,
        rdates,
        exdates,
        recurrence_id,
        recurrence_range: property(component, "RECURRENCE-ID")
            .and_then(|property| parameter(property, "RANGE"))
            .map(str::to_ascii_uppercase),
    })
}

fn parse_datetime_list(
    component: &RawComponent,
    name: &str,
    timezones: &[VTimezone],
    floating_timezone: Tz,
) -> Result<Vec<IcalDateTime>, ()> {
    let mut values = Vec::new();
    for property in properties(component, name) {
        for value in property.value.split(',') {
            if value.contains('/') {
                return Err(());
            }
            let mut property = property.clone();
            property.value = value.trim().to_owned();
            values.push(parse_datetime(&property, timezones, floating_timezone)?);
        }
    }
    Ok(values)
}

fn parse_freebusy(
    component: &RawComponent,
    timezones: &[VTimezone],
    floating_timezone: Tz,
) -> Result<Vec<Interval>, ()> {
    let mut intervals = Vec::new();
    for property in properties(component, "FREEBUSY") {
        let kind = parameter(property, "FBTYPE")
            .unwrap_or("BUSY")
            .to_ascii_uppercase();
        if matches!(kind.as_str(), "FREE" | "BUSY-TENTATIVE") {
            continue;
        }
        if !matches!(kind.as_str(), "BUSY" | "BUSY-UNAVAILABLE") {
            return Err(());
        }
        for period in property.value.split(',') {
            let (start, end) = period.split_once('/').ok_or(())?;
            let start =
                parse_datetime_value(start, property, timezones, floating_timezone)?.to_utc()?;
            let end = if end.starts_with('P') || end.starts_with("-P") {
                start + parse_duration(end)?.to_chrono_duration()?
            } else {
                parse_datetime_value(end, property, timezones, floating_timezone)?.to_utc()?
            };
            if end > start {
                intervals.push(Interval { start, end });
                if intervals.len() > MAX_OCCURRENCES {
                    return Err(());
                }
            }
        }
    }
    Ok(intervals)
}

fn parse_datetime(
    property: &RawProperty,
    timezones: &[VTimezone],
    floating_timezone: Tz,
) -> Result<IcalDateTime, ()> {
    parse_datetime_value(&property.value, property, timezones, floating_timezone)
}

fn parse_datetime_value(
    raw_value: &str,
    property: &RawProperty,
    timezones: &[VTimezone],
    floating_timezone: Tz,
) -> Result<IcalDateTime, ()> {
    let raw_value = raw_value.trim();
    let value_type = parameter(property, "VALUE").unwrap_or("");
    if value_type.eq_ignore_ascii_case("DATE") || (value_type.is_empty() && raw_value.len() == 8) {
        let date = NaiveDate::parse_from_str(raw_value, "%Y%m%d").map_err(|_| ())?;
        return Ok(IcalDateTime {
            value: DateValue::Date(date),
            zone: ResolvedZone::Iana(floating_timezone),
        });
    }
    if let Some(utc_value) = raw_value.strip_suffix('Z') {
        let local = parse_local_datetime(utc_value)?;
        return Ok(IcalDateTime {
            value: DateValue::Utc(DateTime::from_naive_utc_and_offset(local, Utc)),
            zone: ResolvedZone::Iana(chrono_tz::UTC),
        });
    }

    let local = parse_local_datetime(raw_value)?;
    let zone = match parameter(property, "TZID") {
        Some(tzid) => mailrs_ical::vtimezone::resolve(tzid.trim_matches('"'), timezones)
            .map(ResolvedZone::Embedded)
            .ok_or(())?,
        None => ResolvedZone::Iana(floating_timezone),
    };
    Ok(IcalDateTime {
        value: DateValue::DateTime(local),
        zone,
    })
}

fn parse_local_datetime(value: &str) -> Result<NaiveDateTime, ()> {
    ["%Y%m%dT%H%M%S", "%Y%m%dT%H%M"]
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())
        .ok_or(())
}

impl IcalDateTime {
    fn local(&self) -> NaiveDateTime {
        match self.value {
            DateValue::Date(date) => date.and_time(NaiveTime::MIN),
            DateValue::DateTime(local) => local,
            DateValue::Utc(utc) => utc.naive_utc(),
        }
    }

    fn is_date(&self) -> bool {
        matches!(self.value, DateValue::Date(_))
    }

    fn to_utc(&self) -> Result<DateTime<Utc>, ()> {
        match self.value {
            DateValue::Utc(utc) => Ok(utc),
            DateValue::Date(_) | DateValue::DateTime(_) => resolve_local(&self.zone, self.local()),
        }
    }
}

fn resolve_local(zone: &ResolvedZone, local: NaiveDateTime) -> Result<DateTime<Utc>, ()> {
    match zone {
        ResolvedZone::Iana(tz) => match tz.from_local_datetime(&local) {
            LocalResult::Single(date_time) => Ok(date_time.with_timezone(&Utc)),
            LocalResult::Ambiguous(first, second) => Ok(first.min(second).with_timezone(&Utc)),
            LocalResult::None => {
                // RFC 5545 assigns a time in a DST gap to the offset before the gap.
                let mut probe = local;
                for _ in 0..=26 * 60 {
                    probe -= Duration::minutes(1);
                    match tz.from_local_datetime(&probe) {
                        LocalResult::Single(value) => {
                            return Ok(DateTime::from_naive_utc_and_offset(
                                local
                                    - Duration::seconds(i64::from(
                                        value.offset().fix().local_minus_utc(),
                                    )),
                                Utc,
                            ));
                        }
                        LocalResult::Ambiguous(first, second) => {
                            let value = first.min(second);
                            return Ok(DateTime::from_naive_utc_and_offset(
                                local
                                    - Duration::seconds(i64::from(
                                        value.offset().fix().local_minus_utc(),
                                    )),
                                Utc,
                            ));
                        }
                        LocalResult::None => {}
                    }
                }
                Err(())
            }
        },
        ResolvedZone::Embedded(resolved) => {
            let offset = match resolved {
                mailrs_ical::vtimezone::ResolvedTz::Custom(transitions) => {
                    if !(1970..=2050).contains(&local.year()) {
                        return Err(());
                    }
                    embedded_offset(transitions, local)
                        .or_else(|| {
                            mailrs_ical::vtimezone::local_to_utc_offset_seconds(resolved, local)
                        })
                        .ok_or(())?
                }
                mailrs_ical::vtimezone::ResolvedTz::Iana(tz) => {
                    return resolve_local(&ResolvedZone::Iana(*tz), local);
                }
            };
            Ok(DateTime::from_naive_utc_and_offset(
                local - Duration::seconds(i64::from(offset)),
                Utc,
            ))
        }
    }
}

fn embedded_offset(
    transitions: &[mailrs_ical::vtimezone::TzTransition],
    local: NaiveDateTime,
) -> Option<i32> {
    let mut previous = None;
    for transition in transitions {
        if let Some(previous_offset) = previous {
            let gap_seconds = transition.utc_offset_seconds - previous_offset;
            if gap_seconds > 0
                && local >= transition.effective_from
                && local < transition.effective_from + Duration::seconds(i64::from(gap_seconds))
            {
                return Some(previous_offset);
            }
        }
        if transition.effective_from > local {
            break;
        }
        previous = Some(transition.utc_offset_seconds);
    }
    previous
}

impl CalendarDuration {
    fn to_chrono_duration(self) -> Result<Duration, ()> {
        self.days
            .checked_mul(86_400)
            .and_then(|days| days.checked_add(self.seconds))
            .map(Duration::seconds)
            .ok_or(())
    }
}

fn parse_duration(value: &str) -> Result<CalendarDuration, ()> {
    let value = value.trim();
    let (negative, value) = if let Some(value) = value.strip_prefix('-') {
        (true, value)
    } else if let Some(value) = value.strip_prefix('+') {
        (false, value)
    } else {
        (false, value)
    };
    let value = value.strip_prefix('P').ok_or(())?;
    if !value.chars().any(|character| character.is_ascii_digit()) || value.ends_with('T') {
        return Err(());
    }
    let (date_part, time_part) = value.split_once('T').unwrap_or((value, ""));
    let mut duration = CalendarDuration::default();
    parse_duration_fields(
        date_part,
        &[('W', 7 * 86_400), ('D', 86_400)],
        &mut duration,
    )?;
    parse_duration_fields(
        time_part,
        &[('H', 3_600), ('M', 60), ('S', 1)],
        &mut duration,
    )?;
    if negative {
        duration.days = -duration.days;
        duration.seconds = -duration.seconds;
    }
    if duration.days < 0 || duration.seconds < 0 {
        return Err(());
    }
    Ok(duration)
}

fn parse_duration_fields(
    value: &str,
    units: &[(char, i64)],
    out: &mut CalendarDuration,
) -> Result<(), ()> {
    let mut digits = String::new();
    let mut saw = false;
    for ch in value.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        let amount = digits.parse::<i64>().map_err(|_| ())?;
        let multiplier = units
            .iter()
            .find_map(|(unit, multiplier)| (*unit == ch).then_some(*multiplier))
            .ok_or(())?;
        let seconds = amount.checked_mul(multiplier).ok_or(())?;
        if multiplier >= 86_400 {
            out.days = out.days.checked_add(seconds / 86_400).ok_or(())?;
        } else {
            out.seconds = out.seconds.checked_add(seconds).ok_or(())?;
        }
        digits.clear();
        saw = true;
    }
    if !digits.is_empty() || (!saw && !value.is_empty()) {
        return Err(());
    }
    Ok(())
}

fn property<'a>(component: &'a RawComponent, name: &str) -> Option<&'a RawProperty> {
    component
        .properties
        .iter()
        .find(|property| property.name.eq_ignore_ascii_case(name))
}

fn properties<'a>(component: &'a RawComponent, name: &str) -> Vec<&'a RawProperty> {
    component
        .properties
        .iter()
        .filter(|property| property.name.eq_ignore_ascii_case(name))
        .collect()
}

fn parameter<'a>(property: &'a RawProperty, name: &str) -> Option<&'a str> {
    property
        .params
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn unescape_text(value: &str) -> String {
    let mut chars = value.chars();
    let mut output = String::with_capacity(value.len());
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n' | 'N') => output.push('\n'),
                Some(next) => output.push(next),
                None => output.push('\\'),
            }
        } else {
            output.push(ch);
        }
    }
    output
}

pub struct RenderedCalendar {
    pub body: String,
    pub skipped_events: usize,
}

pub fn render(
    sources: &[(usize, ParsedSource)],
    window: Window,
    mode: OutputMode,
    generated_at: DateTime<Utc>,
) -> Result<RenderedCalendar, CalendarError> {
    let mut occurrences = Vec::new();
    let mut intervals = Vec::new();
    let mut skipped_events = 0;

    for (source_index, source) in sources {
        skipped_events += source.skipped_events;
        intervals.extend(
            source
                .freebusy
                .iter()
                .filter_map(|interval| clip_interval(*interval, window)),
        );
        let expanded = expand_source(source, *source_index, window, &mut skipped_events);
        occurrences.extend(expanded);
    }

    let mut calendar = Calendar::new();
    calendar.name("Availability");
    let created_at = generated_at.with_nanosecond(0).unwrap_or(generated_at);

    match mode {
        OutputMode::Busy => {
            intervals.extend(occurrences.into_iter().map(|occurrence| Interval {
                start: occurrence.start,
                end: occurrence.end,
            }));
            for interval in merge_intervals(intervals) {
                let mut event = Event::new();
                event
                    .uid(&stable_uid(&format!(
                        "busy:{}:{}",
                        interval.start.timestamp(),
                        interval.end.timestamp()
                    )))
                    .starts(DatePerhapsTime::DateTime(CalendarDateTime::Utc(
                        interval.start,
                    )))
                    .ends(DatePerhapsTime::DateTime(CalendarDateTime::Utc(
                        interval.end,
                    )))
                    .summary("Busy")
                    .status(icalendar::EventStatus::Confirmed)
                    .timestamp(created_at);
                calendar.push(event.done());
            }
        }
        OutputMode::Title => {
            for occurrence in occurrences {
                let title = if occurrence.private || occurrence.title.trim().is_empty() {
                    "Busy"
                } else {
                    occurrence.title.as_str()
                };
                let mut event = Event::new();
                event
                    .uid(&stable_uid(&format!(
                        "title:{}:{}:{}",
                        occurrence.source_index, occurrence.uid, occurrence.recurrence_key,
                    )))
                    .starts(DatePerhapsTime::DateTime(CalendarDateTime::Utc(
                        occurrence.start,
                    )))
                    .ends(DatePerhapsTime::DateTime(CalendarDateTime::Utc(
                        occurrence.end,
                    )))
                    .summary(title)
                    .status(icalendar::EventStatus::Confirmed)
                    .timestamp(created_at);
                calendar.push(event.done());
            }
            for interval in merge_intervals(intervals) {
                let mut event = Event::new();
                event
                    .uid(&stable_uid(&format!(
                        "freebusy:{}:{}",
                        interval.start.timestamp(),
                        interval.end.timestamp()
                    )))
                    .starts(DatePerhapsTime::DateTime(CalendarDateTime::Utc(
                        interval.start,
                    )))
                    .ends(DatePerhapsTime::DateTime(CalendarDateTime::Utc(
                        interval.end,
                    )))
                    .summary("Busy")
                    .status(icalendar::EventStatus::Confirmed)
                    .timestamp(created_at);
                calendar.push(event.done());
            }
        }
    }

    Ok(RenderedCalendar {
        body: calendar.to_string(),
        skipped_events,
    })
}

fn expand_source(
    source: &ParsedSource,
    source_index: usize,
    window: Window,
    skipped_events: &mut usize,
) -> Vec<Occurrence> {
    let mut by_uid: BTreeMap<&str, Vec<&CalendarEvent>> = BTreeMap::new();
    for event in &source.events {
        by_uid.entry(event.uid.as_str()).or_default().push(event);
    }

    let mut occurrences = Vec::new();
    for events in by_uid.values() {
        let masters = events
            .iter()
            .copied()
            .filter(|event| event.recurrence_id.is_none())
            .collect::<Vec<_>>();
        let overrides = events
            .iter()
            .copied()
            .filter(|event| event.recurrence_id.is_some())
            .collect::<Vec<_>>();
        let master = masters.last().copied();

        if overrides
            .iter()
            .any(|event| event.recurrence_range.as_deref() == Some("THISANDFUTURE"))
        {
            *skipped_events += 1;
            continue;
        }

        if let Some(master) = master {
            match expand_master(master, source_index, window, overrides.as_slice()) {
                Ok(mut expanded) => occurrences.append(&mut expanded),
                Err(()) => *skipped_events += 1,
            }
        } else {
            for override_event in overrides {
                if let Some(start) = &override_event.start {
                    match build_occurrence(
                        override_event,
                        override_event,
                        source_index,
                        start.clone(),
                        None,
                        window,
                    ) {
                        Ok(Some(occurrence)) => occurrences.push(occurrence),
                        Ok(None) => {}
                        Err(()) => *skipped_events += 1,
                    }
                } else {
                    *skipped_events += 1;
                }
            }
        }

        // Duplicate master components are malformed; the last version wins.
        if masters.len() > 1 {
            *skipped_events += masters.len() - 1;
        }
    }
    occurrences
}

fn expand_master(
    master: &CalendarEvent,
    source_index: usize,
    window: Window,
    overrides: &[&CalendarEvent],
) -> Result<Vec<Occurrence>, ()> {
    let start = master.start.as_ref().ok_or(())?;
    let duration = event_duration(master, start)?;
    if duration.days > MAX_EVENT_DURATION_DAYS
        || duration.seconds > MAX_EVENT_DURATION_DAYS * 86_400
    {
        return Err(());
    }

    let candidates = recurrence_candidates(master, start, duration, window)?;
    let overrides_by_start = overrides
        .iter()
        .filter_map(|event| {
            event
                .recurrence_id
                .as_ref()?
                .to_utc()
                .ok()
                .map(|key| (key.timestamp(), *event))
        })
        .collect::<HashMap<_, _>>();
    let mut output = Vec::new();
    let mut handled_overrides = HashSet::new();

    for (original_start, date_time) in candidates {
        if let Some(override_event) = overrides_by_start.get(&original_start) {
            handled_overrides.insert(original_start);
            match build_occurrence(
                override_event,
                master,
                source_index,
                override_event.start.clone().unwrap_or(date_time),
                Some(duration),
                window,
            ) {
                Ok(Some(occurrence)) => output.push(occurrence),
                Ok(None) => {}
                Err(()) => return Err(()),
            }
        } else {
            match build_occurrence(
                master,
                master,
                source_index,
                date_time,
                Some(duration),
                window,
            ) {
                Ok(Some(occurrence)) => output.push(occurrence),
                Ok(None) => {}
                Err(()) => return Err(()),
            }
        }
    }

    // A moved exception can move into the visible window from a recurrence
    // date that is outside it. Add it if it did not match an expanded instance.
    for override_event in overrides {
        let Some(recurrence_id) = &override_event.recurrence_id else {
            continue;
        };
        let Ok(original_start) = recurrence_id.to_utc() else {
            continue;
        };
        if handled_overrides.contains(&original_start.timestamp()) {
            continue;
        }
        if let Some(start) = &override_event.start {
            match build_occurrence(
                override_event,
                master,
                source_index,
                start.clone(),
                Some(duration),
                window,
            ) {
                Ok(Some(occurrence)) => output.push(occurrence),
                Ok(None) => {}
                Err(()) => return Err(()),
            }
        }
    }

    Ok(output)
}

fn recurrence_candidates(
    event: &CalendarEvent,
    start: &IcalDateTime,
    duration: CalendarDuration,
    window: Window,
) -> Result<BTreeMap<i64, IcalDateTime>, ()> {
    let mut candidates = BTreeMap::new();
    let start_utc = start.to_utc()?;

    if let Some(rrule) = &event.rrule {
        let (rrule, until) = split_until(rrule)?;
        let estimated_work = estimated_rule_work(start.local(), window.start.naive_utc(), &rrule)?;
        if estimated_work > MAX_RRULE_WORK {
            return Err(());
        }
        let dtstart = start.local().format("%Y%m%dT%H%M%SZ");
        let input = format!("DTSTART:{dtstart}\nRRULE:{rrule}");
        let rule_set: RRuleSet = input.parse().map_err(|_| ())?;
        let margin = Duration::days(duration.days.clamp(1, MAX_EVENT_DURATION_DAYS) + 1);
        let after_naive = window.start.naive_utc() - margin;
        let before_naive = window.end.naive_utc() + Duration::days(2);
        let after = RRuleTz::UTC.from_utc_datetime(&after_naive);
        let before = RRuleTz::UTC.from_utc_datetime(&before_naive);
        let expanded = rule_set
            .after(after)
            .before(before)
            .all(MAX_OCCURRENCES as u16);
        if expanded.limited {
            return Err(());
        }
        for recurrence in expanded.dates {
            let local = recurrence.naive_utc();
            let value = if start.is_date() {
                DateValue::Date(local.date())
            } else {
                DateValue::DateTime(local)
            };
            let candidate = IcalDateTime {
                value,
                zone: start.zone.clone(),
            };
            if until
                .as_ref()
                .is_some_and(|until| !until.includes(&candidate))
            {
                continue;
            }
            insert_candidate(&mut candidates, candidate)?;
            if candidates.len() > MAX_OCCURRENCES {
                return Err(());
            }
        }
    } else {
        insert_candidate(&mut candidates, start.clone())?;
    }

    for rdate in &event.rdates {
        insert_candidate(&mut candidates, rdate.clone())?;
    }
    let exclusions = event
        .exdates
        .iter()
        .map(IcalDateTime::to_utc)
        .collect::<Result<HashSet<_>, _>>()?;
    // DTSTART participates in an RRULE's recurrence set even if a producer's
    // implementation does not return it in the expansion.
    candidates
        .entry(start_utc.timestamp())
        .or_insert_with(|| start.clone());
    candidates.retain(|timestamp, _| {
        !exclusions
            .iter()
            .any(|excluded| excluded.timestamp() == *timestamp)
    });
    Ok(candidates)
}

fn insert_candidate(
    candidates: &mut BTreeMap<i64, IcalDateTime>,
    date_time: IcalDateTime,
) -> Result<(), ()> {
    let instant = date_time.to_utc()?;
    candidates.insert(instant.timestamp(), date_time);
    if candidates.len() > MAX_OCCURRENCES {
        return Err(());
    }
    Ok(())
}

#[derive(Debug)]
enum Until {
    Utc(DateTime<Utc>),
    Local(NaiveDateTime),
    Date(NaiveDate),
}

impl Until {
    fn includes(&self, date_time: &IcalDateTime) -> bool {
        match self {
            Self::Utc(until) => date_time.to_utc().is_ok_and(|date| date <= *until),
            Self::Local(until) => date_time.local() <= *until,
            Self::Date(until) => date_time.local().date() <= *until,
        }
    }
}

fn split_until(rule: &str) -> Result<(String, Option<Until>), ()> {
    let mut parts = Vec::new();
    let mut until = None;
    for part in rule.split(';') {
        let Some((name, value)) = part.split_once('=') else {
            return Err(());
        };
        if name.eq_ignore_ascii_case("UNTIL") {
            if until.is_some() {
                return Err(());
            }
            until = Some(if let Some(value) = value.strip_suffix('Z') {
                let local = parse_local_datetime(value)?;
                Until::Utc(DateTime::from_naive_utc_and_offset(local, Utc))
            } else if value.len() == 8 {
                Until::Date(NaiveDate::parse_from_str(value, "%Y%m%d").map_err(|_| ())?)
            } else {
                Until::Local(parse_local_datetime(value)?)
            });
        } else {
            parts.push(format!("{}={}", name.to_ascii_uppercase(), value));
        }
    }
    if parts.is_empty() {
        return Err(());
    }
    Ok((parts.join(";"), until))
}

fn estimated_rule_work(
    start: NaiveDateTime,
    window_start: NaiveDateTime,
    rule: &str,
) -> Result<i64, ()> {
    let elapsed = window_start
        .signed_duration_since(start)
        .num_seconds()
        .max(0);
    let mut frequency: Option<i64> = None;
    let mut interval = 1_i64;
    for part in rule.split(';') {
        if let Some(value) = part.strip_prefix("FREQ=") {
            frequency = Some(match value {
                "SECONDLY" => 1,
                "MINUTELY" => 60,
                "HOURLY" => 3_600,
                "DAILY" => 86_400,
                "WEEKLY" => 7 * 86_400,
                "MONTHLY" => 28 * 86_400,
                "YEARLY" => 365 * 86_400,
                _ => return Err(()),
            });
        } else if let Some(value) = part.strip_prefix("INTERVAL=") {
            interval = value.parse::<i64>().map_err(|_| ())?;
            if interval <= 0 {
                return Err(());
            }
        }
    }
    Ok(elapsed / frequency.ok_or(())?.saturating_mul(interval).max(1))
}

fn event_duration(event: &CalendarEvent, start: &IcalDateTime) -> Result<CalendarDuration, ()> {
    if let Some(duration) = event.duration {
        return Ok(duration);
    }
    if let Some(end) = &event.end {
        if start.is_date() && end.is_date() {
            let days = end
                .local()
                .date()
                .signed_duration_since(start.local().date())
                .num_days();
            if days < 0 {
                return Err(());
            }
            return Ok(CalendarDuration { days, seconds: 0 });
        }
        let seconds = end
            .to_utc()?
            .signed_duration_since(start.to_utc()?)
            .num_seconds();
        if seconds < 0 {
            return Err(());
        }
        return Ok(CalendarDuration { days: 0, seconds });
    }
    Ok(CalendarDuration {
        days: i64::from(start.is_date()),
        seconds: 0,
    })
}

fn build_occurrence(
    event: &CalendarEvent,
    master: &CalendarEvent,
    source_index: usize,
    start: IcalDateTime,
    inherited_duration: Option<CalendarDuration>,
    window: Window,
) -> Result<Option<Occurrence>, ()> {
    let duration =
        if event.recurrence_id.is_some() && (event.end.is_some() || event.duration.is_some()) {
            event_duration(event, &start)?
        } else if let Some(duration) = inherited_duration {
            duration
        } else {
            event_duration(event, &start)?
        };
    if duration.days > MAX_EVENT_DURATION_DAYS
        || duration.seconds > MAX_EVENT_DURATION_DAYS * 86_400
    {
        return Err(());
    }

    let start_utc = start.to_utc()?;
    let end_utc = if duration.days > 0 && start.is_date() {
        let end_date = start
            .local()
            .date()
            .checked_add_days(chrono::Days::new(duration.days as u64))
            .ok_or(())?;
        let local_end = IcalDateTime {
            value: DateValue::Date(end_date),
            zone: start.zone.clone(),
        };
        local_end.to_utc()? + Duration::seconds(duration.seconds)
    } else {
        start_utc + duration.to_chrono_duration()?
    };
    if end_utc <= start_utc || start_utc >= window.end || end_utc <= window.start {
        return Ok(None);
    }

    let status = event
        .status
        .as_deref()
        .or(master.status.as_deref())
        .unwrap_or("CONFIRMED");
    let transparency = event
        .transparency
        .as_deref()
        .or(master.transparency.as_deref())
        .unwrap_or("OPAQUE");
    let microsoft_status = event
        .microsoft_busy_status
        .as_deref()
        .or(master.microsoft_busy_status.as_deref())
        .unwrap_or("BUSY");
    if matches!(status, "CANCELLED" | "TENTATIVE")
        || !matches!(status, "CONFIRMED" | "CANCELLED" | "TENTATIVE")
        || transparency == "TRANSPARENT"
        || matches!(microsoft_status, "FREE" | "TENTATIVE")
    {
        return Ok(None);
    }
    if !matches!(
        microsoft_status,
        "BUSY" | "BUSY-UNAVAILABLE" | "OOF" | "WORKINGELSEWHERE"
    ) {
        return Ok(None);
    }

    let class = event
        .class
        .as_deref()
        .or(master.class.as_deref())
        .unwrap_or("PUBLIC");
    let title = event
        .summary
        .as_ref()
        .or(master.summary.as_ref())
        .cloned()
        .unwrap_or_default();
    let recurrence_key = event
        .recurrence_id
        .as_ref()
        .map(IcalDateTime::to_utc)
        .transpose()?
        .unwrap_or(start_utc)
        .timestamp();
    Ok(Some(Occurrence {
        source_index,
        uid: master.uid.clone(),
        recurrence_key,
        start: start_utc.max(window.start),
        end: end_utc.min(window.end),
        title,
        private: matches!(class, "PRIVATE" | "CONFIDENTIAL"),
    }))
}

fn merge_intervals(mut intervals: Vec<Interval>) -> Vec<Interval> {
    intervals.retain(|interval| interval.end > interval.start);
    intervals.sort_by_key(|interval| (interval.start, interval.end));
    let mut merged: Vec<Interval> = Vec::new();
    for interval in intervals {
        if let Some(current) = merged.last_mut() {
            if interval.start <= current.end {
                current.end = current.end.max(interval.end);
                continue;
            }
        }
        merged.push(interval);
    }
    merged
}

fn clip_interval(interval: Interval, window: Window) -> Option<Interval> {
    let start = interval.start.max(window.start);
    let end = interval.end.min(window.end);
    (end > start).then_some(Interval { start, end })
}

fn stable_uid(name: &str) -> String {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> SourceConfig {
        SourceConfig {
            url: url::Url::parse("https://calendar.example.test/feed.ics").unwrap(),
            timezone: None,
        }
    }

    fn window(start: &str, end: &str) -> Window {
        Window {
            start: DateTime::parse_from_rfc3339(start)
                .unwrap()
                .with_timezone(&Utc),
            end: DateTime::parse_from_rfc3339(end)
                .unwrap()
                .with_timezone(&Utc),
        }
    }

    fn generated_at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-03-07T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn apple_recurrences_resolve_embedded_dst_and_exdates() {
        let parsed = ParsedSource::parse(
            include_str!("../../tests/fixtures/apple.ics"),
            &source(),
            chrono_tz::Europe::Berlin,
        )
        .unwrap();
        let rendered = render(
            &[(0, parsed)],
            window("2026-03-06T23:00:00Z", "2026-03-19T23:00:00Z"),
            OutputMode::Title,
            generated_at(),
        )
        .unwrap();

        assert!(
            rendered.body.contains("DTSTART:20260307T140000Z"),
            "{}",
            rendered.body
        );
        assert!(rendered.body.contains("DTSTART:20260308T130000Z"));
        assert!(!rendered.body.contains("DTSTART:20260309T130000Z"));
        assert!(rendered.body.contains("DTSTART:20260315T130000Z"));
        assert!(rendered.body.contains("SUMMARY:Planning\\, review"));
        assert!(rendered.body.contains("SUMMARY:Busy"));
        assert!(!rendered.body.contains("Private appointment"));
        assert!(!rendered.body.contains("apple-weekly@example.test"));
        assert!(mailrs_ical::parse::parse_calendar(&rendered.body).is_ok());
    }

    #[test]
    fn outlook_windows_zone_maps_through_embedded_vtimezone_and_skips_free() {
        let parsed = ParsedSource::parse(
            include_str!("../../tests/fixtures/outlook.ics"),
            &source(),
            chrono_tz::UTC,
        )
        .unwrap();
        let rendered = render(
            &[(1, parsed)],
            window("2026-03-29T00:00:00Z", "2026-03-31T00:00:00Z"),
            OutputMode::Title,
            generated_at(),
        )
        .unwrap();

        assert!(rendered.body.contains("DTSTART:20260330T070000Z"));
        assert!(rendered.body.contains("SUMMARY:Focus time"));
        assert!(!rendered.body.contains("SUMMARY:Lunch"));
        assert!(!rendered.body.contains("SUMMARY:Possible meeting"));
        assert!(!rendered.body.contains("SUMMARY:Reminder"));
    }

    #[test]
    fn vfreebusy_includes_busy_and_unavailable_but_not_free_or_tentative() {
        let parsed = ParsedSource::parse(
            include_str!("../../tests/fixtures/freebusy.ics"),
            &source(),
            chrono_tz::UTC,
        )
        .unwrap();
        assert_eq!(parsed.freebusy.len(), 2);

        let rendered = render(
            &[(0, parsed)],
            window("2026-03-07T00:00:00Z", "2026-03-08T00:00:00Z"),
            OutputMode::Busy,
            generated_at(),
        )
        .unwrap();
        assert_eq!(rendered.body.matches("BEGIN:VEVENT").count(), 2);
        assert!(rendered.body.contains("DTSTART:20260307T100000Z"));
        assert!(rendered.body.contains("DTSTART:20260307T120000Z"));
        assert!(!rendered.body.contains("DTSTART:20260307T140000Z"));
    }

    #[test]
    fn recurrence_overrides_cancel_and_move_instances() {
        let parsed = ParsedSource::parse(
            include_str!("../../tests/fixtures/exceptions.ics"),
            &source(),
            chrono_tz::UTC,
        )
        .unwrap();
        let rendered = render(
            &[(0, parsed)],
            window("2026-11-01T00:00:00Z", "2026-11-20T00:00:00Z"),
            OutputMode::Title,
            generated_at(),
        )
        .unwrap();

        assert_eq!(rendered.body.matches("BEGIN:VEVENT").count(), 2);
        assert!(rendered.body.contains("DTSTART:20261102T140000Z"));
        assert!(rendered.body.contains("DTSTART:20261117T170000Z"));
        assert!(rendered.body.contains("SUMMARY:Moved team sync"));
        assert!(!rendered.body.contains("DTSTART:20261109T140000Z"));
        assert!(!rendered.body.contains("DTSTART:20261116T140000Z"));
    }

    #[test]
    fn busy_mode_merges_touching_intervals_and_stabilizes_uids() {
        let first = Interval {
            start: DateTime::parse_from_rfc3339("2026-03-07T10:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            end: DateTime::parse_from_rfc3339("2026-03-07T11:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        let touching = Interval {
            start: first.end,
            end: DateTime::parse_from_rfc3339("2026-03-07T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        assert_eq!(merge_intervals(vec![touching, first]).len(), 1);

        let empty = ParsedSource {
            events: Vec::new(),
            freebusy: vec![first, touching],
            skipped_events: 0,
            total_events: 0,
        };
        let win = window("2026-03-07T00:00:00Z", "2026-03-08T00:00:00Z");
        let a = render(&[(0, empty.clone())], win, OutputMode::Busy, generated_at()).unwrap();
        let b = render(&[(0, empty)], win, OutputMode::Busy, generated_at()).unwrap();
        assert_eq!(a.body, b.body);
        assert_eq!(a.body.matches("BEGIN:VEVENT").count(), 1);
    }

    #[test]
    fn all_day_dtend_is_exclusive_and_events_are_clipped_to_window() {
        let input = "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:all-day\nDTSTART;VALUE=DATE:20260307\nDTEND;VALUE=DATE:20260308\nSUMMARY:Out\nEND:VEVENT\nEND:VCALENDAR\n";
        let parsed = ParsedSource::parse(input, &source(), chrono_tz::UTC).unwrap();
        let rendered = render(
            &[(0, parsed)],
            window("2026-03-07T12:00:00Z", "2026-03-09T00:00:00Z"),
            OutputMode::Busy,
            generated_at(),
        )
        .unwrap();
        assert!(rendered.body.contains("DTSTART:20260307T120000Z"));
        assert!(rendered.body.contains("DTEND:20260308T000000Z"));
        assert_eq!(rendered.body.matches("BEGIN:VEVENT").count(), 1);
    }

    #[test]
    fn invalid_individual_event_is_counted_while_valid_siblings_are_kept() {
        let input = "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:valid\nDTSTART:20260307T100000Z\nDTEND:20260307T110000Z\nEND:VEVENT\nBEGIN:VEVENT\nUID:bad\nDTSTART;TZID=Unknown/Zone:20260307T100000\nDTEND;TZID=Unknown/Zone:20260307T110000\nEND:VEVENT\nEND:VCALENDAR\n";
        let parsed = ParsedSource::parse(input, &source(), chrono_tz::UTC).unwrap();
        assert_eq!(parsed.events.len(), 1);
        assert_eq!(parsed.skipped_events, 1);
    }

    #[test]
    fn embedded_timezone_gap_uses_the_offset_before_the_gap() {
        let root =
            mailrs_ical::parse::parse_calendar(include_str!("../../tests/fixtures/apple.ics"))
                .unwrap();
        let zones = collect_timezones(&root);
        let zone = mailrs_ical::vtimezone::resolve("America/New_York", &zones).unwrap();
        let local = NaiveDate::from_ymd_opt(2026, 3, 8)
            .unwrap()
            .and_hms_opt(2, 30, 0)
            .unwrap();
        let resolved = resolve_local(&ResolvedZone::Embedded(zone), local).unwrap();
        assert_eq!(resolved.to_rfc3339(), "2026-03-08T07:30:00+00:00");
    }
}
