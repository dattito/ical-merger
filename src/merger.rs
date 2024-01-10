use futures::future::join_all;
use icalendar::{parser::read_calendar, Calendar, CalendarComponent};

use crate::error::{Error, Result};

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

pub async fn urls_to_merged_calendar(urls: Vec<String>) -> Result<Calendar> {
    let calendars = urls
        .into_iter()
        .map(|url| async { url_to_components(url).await });

    let calendar = join_all(calendars)
        .await
        .into_iter()
        .collect::<Result<Vec<Vec<CalendarComponent>>>>()?
        .into_iter()
        .flatten()
        .collect::<Calendar>();

    Ok(calendar)
}
