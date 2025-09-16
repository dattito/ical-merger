use futures::stream::FuturesOrdered;
use futures::StreamExt;
use icalendar::{parser::read_calendar, Calendar, CalendarComponent, Component, Event, EventLike};
use uuid::Uuid;
use chrono::Local;

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
    calendar
        .components
        .into_iter()
        .map(|component| {
            if let Some(event) = component.as_event() {
                let mut new_event = Event::new();

                // Always preserve UID - generate one if missing
                if let Some(uid) = event.get_uid() {
                    new_event.uid(uid);
                } else {
                    // Generate a fallback UID for events without one
                    new_event.uid(&format!("generated-uid-{}", Uuid::new_v4()));
                }

                // Preserve start time if available
                if let Some(start) = event.get_start() {
                    new_event.starts(start);
                }

                // Preserve end time if available
                if let Some(end) = event.get_end() {
                    new_event.ends(end);
                }

                // Preserve status if available
                if let Some(status) = event.get_status() {
                    new_event.status(status);
                }

                // Preserve recurrence rules
                if let Some(rrule) = event.property_value("RRULE") {
                    new_event.add_property("RRULE", rrule);
                }

                // Preserve other important properties that might be needed
                if let Some(created) = event.get_timestamp() {
                    new_event.timestamp(created);
                }

                // Set summary based on status
                new_event.summary(match event.get_status() {
                    Some(status) => match status {
                        icalendar::EventStatus::Confirmed => "Blocked",
                        icalendar::EventStatus::Tentative => "Tentative",
                        icalendar::EventStatus::Cancelled => "Cancelled",
                    },
                    None => "Blocked",
                });

                CalendarComponent::Event(new_event.done())
            } else {
                // Preserve non-event components (VTIMEZONE, VTODO, etc.)
                component
            }
        })
        .collect::<Calendar>()
}

pub fn filter_future_days(calendar: Calendar, days_limit: u32) -> Calendar {
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
                            let max_past_days = 90; // Only look back 90 days for recurring events
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
