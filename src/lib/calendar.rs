use chrono::{DateTime, Duration, Utc};
use futures::future::join_all;
use icalendar::{
    parser::read_calendar, Calendar, CalendarComponent, CalendarDateTime, Component,
    DatePerhapsTime, Event, EventLike,
};

use crate::lib::error::{Error, Result};

async fn url_to_text(url: String) -> Result<String> {
    let res = reqwest::get(url).await?.error_for_status()?;

    res.text().await.map_err(Error::Reqwest)
}

fn text_to_calender(text: String) -> Result<Calendar> {
    let text_unfolded = icalendar::parser::unfold(&text);
    let parsed_calender = read_calendar(&text_unfolded).map_err(Error::ParseCalender)?;

    let calendar = Calendar::from(parsed_calender);

    Ok(calendar)
}

async fn url_to_components(url: String) -> Result<Vec<CalendarComponent>> {
    let text = url_to_text(url).await?;

    Ok(text_to_calender(text)?.components)
}

pub async fn urls_to_merged_calendar(urls: Vec<String>, offsets: &Vec<i64>) -> Result<Calendar> {
    let calendars = urls.into_iter().enumerate().map(|(index, url)| async move {
        let components = url_to_components(url).await?;

        if offsets.is_empty() {
            Ok(components)
        } else if index >= offsets.len() {
            Ok(shift_timezone(components, *offsets.last().unwrap()).components)
        } else {
            Ok(shift_timezone(components, *offsets.get(index).unwrap()).components)
        }
    });

    let calendar = join_all(calendars)
        .await
        .into_iter()
        .collect::<Result<Vec<Vec<CalendarComponent>>>>()?
        .into_iter()
        .flatten()
        .collect::<Calendar>();

    Ok(calendar)
}

pub async fn calendars_to_merged_calendar(calendars: Vec<Calendar>) -> Calendar {
    calendars
        .into_iter()
        .flat_map(|c| c.components)
        .collect::<Calendar>()
}

pub fn hide_details(calendar: Calendar) -> Calendar {
    calendar
        .components
        .iter()
        .filter_map(|component| {
            if let Some(event) = component.as_event() {
                let mut new_event = Event::new();

                if let Some(start) = event.get_start() {
                    new_event.starts(start);
                }

                if let Some(end) = event.get_end() {
                    new_event.ends(end);
                }

                if let Some(status) = event.get_status() {
                    new_event.status(status);
                }

                if let Some(uid) = event.get_uid() {
                    new_event.uid(uid);
                }

                if let Some(rrule) = event.property_value("RRULE") {
                    new_event.add_property("RRULE", rrule);
                }

                new_event.summary(match event.get_status() {
                    Some(status) => match status {
                        icalendar::EventStatus::Confirmed => "Blocked",
                        icalendar::EventStatus::Tentative => "Tentative",
                        icalendar::EventStatus::Cancelled => "Cancelled",
                    },
                    None => "Blocked",
                });

                Some(new_event)
            } else {
                None
            }
        })
        .collect::<Calendar>()
}

pub fn shift_timezone(components: Vec<CalendarComponent>, offset: i64) -> Calendar {
    components
        .into_iter()
        .map(|component| {
            if let Some(event) = component.as_event() {
                let mut new_event = event.clone();

                if let Some(starts) = event.get_start() {
                    new_event.starts(shift_date_pehaps_time(starts, offset));
                }

                if let Some(ends) = event.get_end() {
                    new_event.ends(shift_date_pehaps_time(ends, offset));
                }

                CalendarComponent::Event(new_event)
            } else {
                component
            }
        })
        .collect::<Calendar>()
}

fn shift_date_pehaps_time(dpt: DatePerhapsTime, offset: i64) -> DatePerhapsTime {
    if offset == 0 {
        return dpt;
    }

    match dpt {
        DatePerhapsTime::DateTime(dt) => match dt {
            CalendarDateTime::Floating(f) => {
                DatePerhapsTime::DateTime(CalendarDateTime::Floating(f + Duration::hours(offset)))
            }
            CalendarDateTime::Utc(t) => {
                DatePerhapsTime::DateTime(CalendarDateTime::Utc(t + Duration::hours(offset)))
            }
            CalendarDateTime::WithTimezone { date_time, tzid } => {
                DatePerhapsTime::DateTime(CalendarDateTime::WithTimezone {
                    date_time: date_time + Duration::hours(offset),
                    tzid,
                })
            }
        },
        DatePerhapsTime::Date(d) => DatePerhapsTime::Date(d),
    }
}

// makes only sense if hiding details
pub fn merge_all_overlapping_events(calendar: &mut Calendar) {
    let mut new_components: Vec<CalendarComponent> = Vec::new();

    calendar.components.iter().for_each(|component_a| {
        if let CalendarComponent::Event(event_a) = component_a {
            let overlapped_event_index =
                new_components
                    .iter()
                    .position(|component_b| match component_b {
                        CalendarComponent::Event(event_b) => check_events_overlap(event_a, event_b),
                        _ => false,
                    });

            match overlapped_event_index {
                None => new_components.push(CalendarComponent::Event(event_a.clone())),
                Some(oei) => {
                    if let CalendarComponent::Event(event_b) = new_components.get_mut(oei).unwrap()
                    {
                        merge_events(event_b, event_a);
                    }
                }
            };
        } else {
            new_components.push(component_a.clone());
        };
    });

    calendar.components = new_components;
}

fn datetime(dpt: DatePerhapsTime) -> Option<DateTime<Utc>> {
    match dpt {
        DatePerhapsTime::Date(_) => None,
        DatePerhapsTime::DateTime(dt) => match dt {
            CalendarDateTime::Utc(u) => Some(u),
            CalendarDateTime::WithTimezone { date_time, .. } => {
                Some(date_time.and_local_timezone(Utc).unwrap())
            }
            CalendarDateTime::Floating(f) => Some(f.and_local_timezone(Utc).unwrap()),
        },
    }
}

fn check_events_overlap(event_a: &Event, event_b: &Event) -> bool {
    let Some(start_a) = event_a.get_start().and_then(datetime) else {
        return false;
    };
    let Some(end_a) = event_a.get_end().and_then(datetime) else {
        return false;
    };
    let Some(start_b) = event_b.get_start().and_then(datetime) else {
        return false;
    };
    let Some(end_b) = event_b.get_end().and_then(datetime) else {
        return false;
    };

    (start_a <= start_b && start_b <= end_a)
        || (start_b <= start_a && start_a <= end_b)
        || (start_a <= end_b && end_b <= end_a)
        || (start_b <= end_a && end_a <= end_b)
        || ((start_a - start_b).abs() < Duration::minutes(5))
        || ((start_a - end_b).abs() < Duration::minutes(5))
        || ((end_b - start_a).abs() < Duration::minutes(5))
        || ((end_a - end_b).abs() < Duration::minutes(5))
}

// it is assumed that datetime exists for start and end of both events and it is known that the
// events overlap
fn merge_events(event_a: &mut Event, event_b: &Event) {
    let start_a = event_a.get_start().and_then(datetime).unwrap();
    let end_a = event_a.get_end().and_then(datetime).unwrap();

    let start_b = event_b.get_start().and_then(datetime).unwrap();
    let end_b = event_b.get_end().and_then(datetime).unwrap();

    if start_b < start_a {
        event_a.starts(event_b.get_start().unwrap());
    }

    if end_b > end_a {
        event_a.ends(event_b.get_end().unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use icalendar::{Calendar, Event, EventLike};

    // Cases: Important: Event A should end before or at the same time as Event B
    // 1: Event A starts before Event B starts and ends after Event B starts and ends before Event B ends
    // 2: Event A starts after Event B starts and ends after Event B ends
    // 3: Event A starts after Event B starts and ends before Event B ends
    // 4: Event A starts when Event B starts
    // 5: Event A starts when Event B ends
    // 6: Event A ends when Event B ends
    // 7: Event A ends when Event B starts
    //
    // NEGATIVE:
    // 1: Event A starts before Event B starts and ends after Event B starts

    #[test]
    fn case_1() {
        let mut calendar = Calendar::new();
        calendar
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T10:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T12:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            )
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T11:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T13:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            );

        merge_all_overlapping_events(&mut calendar);
        assert_eq!(calendar.components.len(), 1);
    }

    #[test]
    fn case_2() {
        let mut calendar = Calendar::new();
        calendar
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T11:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T13:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            )
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T10:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T12:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            );

        merge_all_overlapping_events(&mut calendar);
        assert_eq!(calendar.components.len(), 1);
    }

    #[test]
    fn case_3() {
        let mut calendar = Calendar::new();
        calendar
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T11:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T12:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            )
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T10:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T13:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            );

        merge_all_overlapping_events(&mut calendar);
        assert_eq!(calendar.components.len(), 1);
    }

    #[test]
    fn case_4() {
        let mut calendar = Calendar::new();
        calendar
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T10:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T11:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            )
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T10:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T12:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            );

        merge_all_overlapping_events(&mut calendar);
        assert_eq!(calendar.components.len(), 1);
    }

    #[test]
    fn case_5() {
        let mut calendar = Calendar::new();
        calendar
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T11:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T12:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            )
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T10:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T11:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            );

        merge_all_overlapping_events(&mut calendar);
        assert_eq!(calendar.components.len(), 1);
    }

    #[test]
    fn case_6() {
        let mut calendar = Calendar::new();
        calendar
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T10:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T12:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            )
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T11:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T12:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            );

        merge_all_overlapping_events(&mut calendar);
        assert_eq!(calendar.components.len(), 1);
    }

    #[test]
    fn custom_1() {
        let mut calendar = Calendar::new();
        calendar
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T10:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T14:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            )
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T11:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T12:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            )
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T14:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T18:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            )
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T15:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T16:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            );

        assert_eq!(calendar.components.len(), 4);
        merge_all_overlapping_events(&mut calendar);
        assert_eq!(calendar.components.len(), 1);
    }
    #[test]
    fn custom_2() {
        let mut calendar = Calendar::new();
        calendar
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T10:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T14:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            )
            .push(
                Event::new()
                    .starts::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T14:02:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .ends::<DateTime<Utc>>(
                        DateTime::parse_from_rfc3339("2024-01-01T16:00:00+02:00")
                            .unwrap()
                            .into(),
                    )
                    .done(),
            );

        merge_all_overlapping_events(&mut calendar);
        assert_eq!(calendar.components.len(), 1);
    }
}
