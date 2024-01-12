use chrono::Duration;
use futures::future::join_all;
use icalendar::{parser::read_calendar, Calendar, CalendarComponent, Component, Event, EventLike, DatePerhapsTime, CalendarDateTime};

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
    let calendars = urls
        .into_iter()
        .enumerate()
        .map(|(index,url)| async move {
           let components = url_to_components(url).await?;

            if offsets.is_empty() {
                println!("test");
                Ok(components)
            } else if index >= offsets.len() {
                println!("xx{offsets:?}");
                Ok(shift_timezone(components, *offsets.last().unwrap()).components)
            } else {
                println!("=={offsets:?}");
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
    calendars.into_iter().flat_map(|c|c.components).collect::<Calendar>()
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

                new_event.summary(match event.get_status() {
                    Some(status) => match status {
                        icalendar::EventStatus::Confirmed => "Blocked",
                        icalendar::EventStatus::Tentative => "Tentative",
                        icalendar::EventStatus::Cancelled => "Cancelled",
                    },
                    None => "Blocked",
                });

                Some(new_event.done())
            } else {
                None
            }
        })
        .collect::<Calendar>()
}

pub fn shift_timezone(components: Vec<CalendarComponent>, offset: i64) -> Calendar {
    components.into_iter().map(|component| {
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
    }).collect::<Calendar>()
}

fn shift_date_pehaps_time(dpt: DatePerhapsTime, offset: i64) -> DatePerhapsTime {

    if offset != 0 {
        println!("{offset}");
    }

    match dpt {
        DatePerhapsTime::DateTime(dt) =>{
            match dt {
                CalendarDateTime::Floating(f) =>DatePerhapsTime::DateTime(CalendarDateTime::Floating(f + Duration::hours(offset))),
                CalendarDateTime::Utc(t) =>DatePerhapsTime::DateTime(CalendarDateTime::Utc(t + Duration::hours(offset))),
                CalendarDateTime::WithTimezone { date_time, tzid } =>DatePerhapsTime::DateTime(CalendarDateTime::WithTimezone { date_time: date_time + Duration::hours(offset), tzid })
            }
        },
        DatePerhapsTime::Date(d) => DatePerhapsTime::Date(d)
    }
}
