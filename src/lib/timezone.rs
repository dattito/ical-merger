use chrono::Duration;
use icalendar::{CalendarComponent, CalendarDateTime, Component, DatePerhapsTime, Event, EventLike};

fn adjust_calendar_datetime_with_offset(calendar_dt: &CalendarDateTime, offset_hours: i64) -> CalendarDateTime {
    if offset_hours == 0 {
        return calendar_dt.clone();
    }

    let offset_duration = Duration::hours(offset_hours);
    match calendar_dt {
        CalendarDateTime::Floating(naive_dt) => {
            if let Some(adjusted_dt) = naive_dt.checked_add_signed(offset_duration) {
                CalendarDateTime::Floating(adjusted_dt)
            } else {
                calendar_dt.clone()
            }
        }
        CalendarDateTime::Utc(utc_dt) => {
            if let Some(adjusted_dt) = utc_dt.checked_add_signed(offset_duration) {
                CalendarDateTime::Utc(adjusted_dt)
            } else {
                calendar_dt.clone()
            }
        }
        CalendarDateTime::WithTimezone { date_time, tzid } => {
            if let Some(adjusted_dt) = date_time.checked_add_signed(offset_duration) {
                CalendarDateTime::WithTimezone {
                    date_time: adjusted_dt,
                    tzid: tzid.clone(),
                }
            } else {
                calendar_dt.clone()
            }
        }
    }
}

fn adjust_dateperhapstime_with_offset(dt: &DatePerhapsTime, offset_hours: i64) -> DatePerhapsTime {
    if offset_hours == 0 {
        return dt.clone();
    }

    match dt {
        DatePerhapsTime::DateTime(calendar_dt) => {
            let adjusted_calendar_dt = adjust_calendar_datetime_with_offset(calendar_dt, offset_hours);
            DatePerhapsTime::DateTime(adjusted_calendar_dt)
        }
        DatePerhapsTime::Date(date) => {
            DatePerhapsTime::Date(*date)
        }
    }
}

pub fn shift_timezone(components: Vec<CalendarComponent>, offset: i64) -> icalendar::Calendar {
    components
        .into_iter()
        .map(|component| {
            if let Some(event) = component.as_event() {
                let mut new_event = Event::new();

                if let Some(uid) = event.get_uid() {
                    new_event.uid(uid);
                }

                if let Some(summary) = event.get_summary() {
                    new_event.summary(summary);
                }

                if let Some(description) = event.get_description() {
                    new_event.description(description);
                }

                if let Some(location) = event.get_location() {
                    new_event.location(location);
                }

                if let Some(status) = event.get_status() {
                    new_event.status(status);
                }

                if let Some(start) = event.get_start() {
                    let adjusted_start = adjust_dateperhapstime_with_offset(&start, offset);
                    new_event.starts(adjusted_start);
                }

                if let Some(end) = event.get_end() {
                    let adjusted_end = adjust_dateperhapstime_with_offset(&end, offset);
                    new_event.ends(adjusted_end);
                }

                if let Some(created) = event.get_timestamp() {
                    if offset != 0 {
                        let offset_duration = Duration::hours(offset);
                        if let Some(adjusted_timestamp) = created.checked_add_signed(offset_duration) {
                            new_event.timestamp(adjusted_timestamp);
                        } else {
                            new_event.timestamp(created);
                        }
                    } else {
                        new_event.timestamp(created);
                    }
                }

                for (key, value) in event.properties() {
                    match key.as_str() {
                        "UID" | "SUMMARY" | "DESCRIPTION" | "LOCATION" | "STATUS" |
                        "DTSTART" | "DTEND" | "DTSTAMP" => {
                        }
                        _ => {
                            new_event.add_property(key, value.value());
                        }
                    }
                }

                CalendarComponent::Event(new_event.done())
            } else {
                component
            }
        })
        .collect::<icalendar::Calendar>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use icalendar::{Calendar, CalendarDateTime, DatePerhapsTime};

    #[test]
    fn test_timezone_shift_positive_offset() {
        // Create a test event with floating time
        let mut event = Event::new();
        event.uid("test-event-1");
        event.summary("Test Meeting");

        let naive_dt = NaiveDate::from_ymd_opt(2024, 1, 1)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap();

        event.starts(DatePerhapsTime::DateTime(CalendarDateTime::Floating(naive_dt)));

        let mut calendar = Calendar::new();
        calendar.push(event.done());

        // Shift by +2 hours
        let shifted_calendar = shift_timezone(calendar.components, 2);

        // Check that we got back a calendar with components
        assert_eq!(shifted_calendar.components.len(), 1);

        // Get the shifted event
        if let Some(shifted_event) = shifted_calendar.components[0].as_event() {
            if let Some(DatePerhapsTime::DateTime(CalendarDateTime::Floating(shifted_dt))) = shifted_event.get_start() {
                // Should be 12:00:00 (10:00 + 2 hours)
                let expected_dt = NaiveDate::from_ymd_opt(2024, 1, 1)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap();
                assert_eq!(shifted_dt, expected_dt);
            } else {
                panic!("Expected floating datetime");
            }
        } else {
            panic!("Expected event component");
        }
    }

    #[test]
    fn test_timezone_shift_negative_offset() {
        let mut event = Event::new();
        event.uid("test-event-2");
        event.summary("Test Meeting 2");

        let naive_dt = NaiveDate::from_ymd_opt(2024, 1, 1)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap();

        event.starts(DatePerhapsTime::DateTime(CalendarDateTime::Floating(naive_dt)));

        let mut calendar = Calendar::new();
        calendar.push(event.done());

        // Shift by -3 hours
        let shifted_calendar = shift_timezone(calendar.components, -3);

        if let Some(shifted_event) = shifted_calendar.components[0].as_event() {
            if let Some(DatePerhapsTime::DateTime(CalendarDateTime::Floating(shifted_dt))) = shifted_event.get_start() {
                // Should be 07:00:00 (10:00 - 3 hours)
                let expected_dt = NaiveDate::from_ymd_opt(2024, 1, 1)
                    .unwrap()
                    .and_hms_opt(7, 0, 0)
                    .unwrap();
                assert_eq!(shifted_dt, expected_dt);
            } else {
                panic!("Expected floating datetime");
            }
        } else {
            panic!("Expected event component");
        }
    }

    #[test]
    fn test_timezone_shift_zero_offset() {
        let mut event = Event::new();
        event.uid("test-event-3");
        event.summary("Test Meeting 3");

        let naive_dt = NaiveDate::from_ymd_opt(2024, 1, 1)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap();

        event.starts(DatePerhapsTime::DateTime(CalendarDateTime::Floating(naive_dt)));

        let mut calendar = Calendar::new();
        calendar.push(event.done());

        // Shift by 0 hours (no change)
        let shifted_calendar = shift_timezone(calendar.components, 0);

        if let Some(shifted_event) = shifted_calendar.components[0].as_event() {
            if let Some(DatePerhapsTime::DateTime(CalendarDateTime::Floating(shifted_dt))) = shifted_event.get_start() {
                // Should remain 10:00:00
                assert_eq!(shifted_dt, naive_dt);
            } else {
                panic!("Expected floating datetime");
            }
        } else {
            panic!("Expected event component");
        }
    }

    #[test]
    fn test_timezone_shift_with_utc_datetime() {
        let mut event = Event::new();
        event.uid("test-event-4");
        event.summary("UTC Test Meeting");

        let utc_dt = NaiveDate::from_ymd_opt(2024, 1, 1)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap()
            .and_utc();

        event.starts(DatePerhapsTime::DateTime(CalendarDateTime::Utc(utc_dt)));

        let mut calendar = Calendar::new();
        calendar.push(event.done());

        // Shift by +1 hour
        let shifted_calendar = shift_timezone(calendar.components, 1);

        if let Some(shifted_event) = shifted_calendar.components[0].as_event() {
            if let Some(DatePerhapsTime::DateTime(CalendarDateTime::Utc(shifted_dt))) = shifted_event.get_start() {
                // Should be 11:00:00 UTC
                let expected_dt = NaiveDate::from_ymd_opt(2024, 1, 1)
                    .unwrap()
                    .and_hms_opt(11, 0, 0)
                    .unwrap()
                    .and_utc();
                assert_eq!(shifted_dt, expected_dt);
            } else {
                panic!("Expected UTC datetime");
            }
        } else {
            panic!("Expected event component");
        }
    }
}