use futures::stream::FuturesOrdered;
use futures::StreamExt;
use icalendar::{parser::read_calendar, Calendar, CalendarComponent, Component, Event, EventLike, DatePerhapsTime, CalendarDateTime};
use uuid::Uuid;
use chrono::{Local, NaiveDateTime, Weekday, Datelike};

use crate::lib::error::{Error, Result};
use crate::lib::timezone::shift_timezone;

async fn url_to_text(url: String) -> Result<String> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(Error::Reqwest)?;

    let res = client
        .get(&url)
        .header("Accept", "text/calendar,application/calendar,text/plain,*/*")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .send()
        .await
        .map_err(Error::Reqwest)?;

    if !res.status().is_success() {
        return Err(Error::ParseCalender(format!(
            "HTTP {} error for URL {}: {}",
            res.status(),
            url,
            res.text().await.unwrap_or_else(|_| "Unknown error".to_string())
        )));
    }

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

pub async fn urls_to_merged_calendar(urls: Vec<String>, offsets: &[i64]) -> Result<Calendar> {
    let calendar = urls
        .into_iter()
        .enumerate()
        .map(|(index, url)| async move {
            let components = url_to_components(url).await?;

            if offsets.is_empty() {
                Ok(components)
            } else if index >= offsets.len() {
                Ok(shift_timezone(components, *offsets.last().unwrap()).components)
            } else {
                Ok(shift_timezone(components, *offsets.get(index).unwrap()).components)
            }
        })
        .collect::<FuturesOrdered<_>>()
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Calendar>();

    // let calendar = join_all(calendars)
    //     .await
    //     .into_iter()
    //     .collect::<Result<Vec<Vec<CalendarComponent>>>>()?
    //     .into_iter()
    //     .flatten()
    //     .collect::<Calendar>();

    Ok(calendar)
}

pub async fn calendars_to_merged_calendar(calendars: Vec<Calendar>) -> Calendar {
    calendars
        .into_iter()
        .flat_map(|c| c.components)
        .collect::<Calendar>()
}

pub fn hide_details(calendar: Calendar) -> Calendar {
    let mut all_event_slots = Vec::new();
    let mut non_events = Vec::new();

    // Define the 14-day window for recurring event expansion
    let window_start = Local::now().naive_local();
    let window_end = window_start + chrono::Duration::days(14);

    // Process all events and expand recurring ones
    for component in calendar.components {
        if let Some(event) = component.as_event() {
            // Only process events that have both start and end times
            if let (Some(start), Some(end)) = (event.get_start(), event.get_end()) {
                if let (Some(start_dt), Some(end_dt)) = (extract_naive_datetime(&start), extract_naive_datetime(&end)) {
                    let uid = event.get_uid().map(|s| s.to_string()).unwrap_or_else(|| format!("generated-uid-{}", Uuid::new_v4()));

                    // Check if this is a recurring event
                    if let Some(rrule) = event.property_value("RRULE") {
                        // Expand recurring event into individual occurrences
                        let expanded_occurrences = expand_recurring_event(
                            start_dt,
                            end_dt,
                            rrule,
                            window_start,
                            window_end,
                        );
                        all_event_slots.extend(expanded_occurrences);
                    } else {
                        // For single events, add directly
                        all_event_slots.push(EventTimeSlot {
                            start: start_dt,
                            end: end_dt,
                            uid,
                        });
                    }
                }
            }
        } else {
            // Keep non-event components (VTIMEZONE, etc.)
            non_events.push(component);
        }
    }

    // Merge ALL overlapping events (both single and expanded recurring)
    let merged_events = merge_overlapping_events(all_event_slots);

    // Create new calendar with merged events
    let mut calendar_components = non_events;

    for event_slot in merged_events {
        let mut new_event = Event::new();

        new_event.uid(&event_slot.uid);
        new_event.starts(DatePerhapsTime::DateTime(CalendarDateTime::Floating(event_slot.start)));
        new_event.ends(DatePerhapsTime::DateTime(CalendarDateTime::Floating(event_slot.end)));
        new_event.summary("Blocked");
        new_event.status(icalendar::EventStatus::Confirmed);

        calendar_components.push(CalendarComponent::Event(new_event.done()));
    }

    calendar_components.into_iter().collect::<Calendar>()
}

fn extract_naive_datetime(date_time: &DatePerhapsTime) -> Option<NaiveDateTime> {
    match date_time {
        DatePerhapsTime::DateTime(calendar_dt) => {
            match calendar_dt {
                CalendarDateTime::Floating(naive_dt) => Some(*naive_dt),
                CalendarDateTime::Utc(utc_dt) => Some(utc_dt.naive_local()),
                CalendarDateTime::WithTimezone { date_time, .. } => Some(*date_time),
            }
        },
        DatePerhapsTime::Date(date) => {
            // Convert date to datetime at start of day
            Some(date.and_hms_opt(0, 0, 0)?)
        },
    }
}

#[derive(Debug, Clone)]
struct EventTimeSlot {
    start: NaiveDateTime,
    end: NaiveDateTime,
    uid: String,
}

fn parse_weekday(day_str: &str) -> Option<Weekday> {
    match day_str {
        "MO" => Some(Weekday::Mon),
        "TU" => Some(Weekday::Tue),
        "WE" => Some(Weekday::Wed),
        "TH" => Some(Weekday::Thu),
        "FR" => Some(Weekday::Fri),
        "SA" => Some(Weekday::Sat),
        "SU" => Some(Weekday::Sun),
        _ => None,
    }
}

fn expand_recurring_event(
    start_dt: NaiveDateTime,
    end_dt: NaiveDateTime,
    rrule: &str,
    window_start: NaiveDateTime,
    window_end: NaiveDateTime,
) -> Vec<EventTimeSlot> {
    let mut occurrences = Vec::new();

    // Parse RRULE - simple implementation for WEEKLY events
    if !rrule.contains("FREQ=WEEKLY") {
        // For non-weekly events, just return the original if it's in the window
        if start_dt >= window_start && start_dt <= window_end {
            occurrences.push(EventTimeSlot {
                start: start_dt,
                end: end_dt,
                uid: format!("expanded-{}", Uuid::new_v4()),
            });
        }
        return occurrences;
    }

    // Parse BYDAY parameter
    let weekdays: Vec<Weekday> = if let Some(byday_part) = rrule.split(';').find(|part| part.starts_with("BYDAY=")) {
        byday_part
            .strip_prefix("BYDAY=")
            .unwrap_or("")
            .split(',')
            .filter_map(parse_weekday)
            .collect()
    } else {
        // If no BYDAY specified, use the original event's weekday
        vec![start_dt.weekday()]
    };

    // Generate occurrences for each weekday in the window
    for target_weekday in weekdays {
        let mut current_date = window_start.date();

        // Find the first occurrence of target_weekday on or after window_start
        while current_date.weekday() != target_weekday {
            current_date = current_date.succ_opt().unwrap_or(current_date);
            if current_date > window_end.date() {
                break;
            }
        }

        // Generate weekly occurrences
        while current_date <= window_end.date() {
            let occurrence_start = current_date.and_time(start_dt.time());
            let occurrence_end = current_date.and_time(end_dt.time());

            if occurrence_start >= window_start && occurrence_start <= window_end {
                occurrences.push(EventTimeSlot {
                    start: occurrence_start,
                    end: occurrence_end,
                    uid: format!("expanded-{}", Uuid::new_v4()),
                });
            }

            // Move to next week
            current_date += chrono::Duration::weeks(1);
        }
    }

    occurrences
}

fn merge_overlapping_events(events: Vec<EventTimeSlot>) -> Vec<EventTimeSlot> {
    if events.is_empty() {
        return events;
    }

    // Sort events by start time
    let mut sorted_events = events;
    sorted_events.sort_by_key(|e| e.start);

    let mut merged = Vec::new();
    let mut current = sorted_events[0].clone();

    for event in sorted_events.into_iter().skip(1) {
        // Check for overlap: current event hasn't ended when next event starts
        if current.end >= event.start {
            // Overlap detected - merge them
            // Take the earliest start and latest end
            current.start = current.start.min(event.start);
            current.end = current.end.max(event.end);
            // Keep the first UID for the merged event
        } else {
            // No overlap - save current and move to next
            merged.push(current);
            current = event;
        }
    }

    // Don't forget the last event
    merged.push(current);
    merged
}

pub fn filter_future_days(calendar: Calendar, days_limit: u32) -> Calendar {
    let hide_details_mode = std::env::var("HIDE_DETAILS").unwrap_or_default().to_lowercase() == "true";
    let today = Local::now().date_naive();
    let end_date = today + chrono::Duration::days(days_limit as i64);

    calendar
        .components
        .into_iter()
        .filter_map(|component| {
            match component.as_event() {
                Some(event) => {
                    // Check if event has a start date
                    if let Some(start) = event.get_start() {
                        let event_date = match start {
                            icalendar::DatePerhapsTime::DateTime(calendar_dt) => {
                                match calendar_dt {
                                    icalendar::CalendarDateTime::Floating(naive_dt) => naive_dt.date(),
                                    icalendar::CalendarDateTime::Utc(utc_dt) => utc_dt.naive_local().date(),
                                    icalendar::CalendarDateTime::WithTimezone { date_time, .. } => date_time.date(),
                                }
                            },
                            icalendar::DatePerhapsTime::Date(date) => date,
                        };

                        // Check if this is a recurring event
                        if let Some(rrule) = event.property_value("RRULE") {
                            // For recurring events, only include if:
                            // 1. They start within our future window, OR
                            // 2. They started recently enough to have current/future occurrences
                            let max_past_days = if hide_details_mode {
                                // In privacy mode, be more restrictive - only look back 1 day
                                // to avoid "mysterious" events from distant past
                                1
                            } else {
                                // Normal mode - look back 90 days for recurring events
                                90
                            };
                            if event_date >= today - chrono::Duration::days(max_past_days) && event_date <= end_date {
                                // Create a new event with truncated RRULE
                                let mut new_event = Event::new();

                                // Copy essential properties
                                if let Some(uid) = event.get_uid() {
                                    new_event.uid(uid);
                                } else {
                                    new_event.uid(&format!("generated-uid-{}", Uuid::new_v4()));
                                }

                                if let Some(start) = event.get_start() {
                                    new_event.starts(start);
                                }

                                if let Some(end) = event.get_end() {
                                    new_event.ends(end);
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

                                if let Some(created) = event.get_timestamp() {
                                    new_event.timestamp(created);
                                }

                                // Copy other properties that might exist
                                for prop_name in ["CLASS", "PRIORITY", "SEQUENCE", "TRANSP", "CATEGORIES", "RELATED-TO", "RECURRENCE-ID", "EXDATE"] {
                                    if let Some(prop_value) = event.property_value(prop_name) {
                                        new_event.add_property(prop_name, prop_value);
                                    }
                                }

                                // Create a truncated RRULE that ends at our date limit
                                let end_datetime = end_date.and_hms_opt(23, 59, 59).unwrap_or_else(|| {
                                    end_date.and_hms_opt(0, 0, 0).unwrap()
                                });
                                let end_datetime_str = end_datetime.format("%Y%m%dT%H%M%SZ").to_string();

                                // Parse existing RRULE and modify it
                                let truncated_rrule = if rrule.contains("UNTIL=") {
                                    // Replace existing UNTIL with our limit
                                    let parts: Vec<&str> = rrule.split(';').collect();
                                    let new_parts: Vec<String> = parts.iter().map(|part| {
                                        if part.starts_with("UNTIL=") {
                                            format!("UNTIL={}", end_datetime_str)
                                        } else {
                                            part.to_string()
                                        }
                                    }).collect();
                                    new_parts.join(";")
                                } else {
                                    // Add UNTIL to existing RRULE
                                    format!("{};UNTIL={}", rrule, end_datetime_str)
                                };

                                new_event.add_property("RRULE", &truncated_rrule);
                                Some(CalendarComponent::Event(new_event.done()))
                            } else {
                                // Recurring event starts after our window, exclude it
                                None
                            }
                        } else {
                            // Non-recurring event: include only if in window
                            if event_date >= today && event_date <= end_date {
                                Some(component)
                            } else {
                                None
                            }
                        }
                    } else {
                        // Keep events without start date (shouldn't happen, but be safe)
                        Some(component)
                    }
                },
                None => {
                    // Keep non-event components (VTIMEZONE, etc.)
                    Some(component)
                }
            }
        })
        .collect::<Calendar>()
}
