use axum::{http::StatusCode, response::IntoResponse};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("failed to make request: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("failed to parse calender: {0}")]
    ParseCalender(String),

    #[error("environment variables could not be validated: {0:#?}")]
    Envy(#[from] envy::Error),

    #[error("cannot bind tcp port: {0}")]
    IO(#[from] std::io::Error),
}

impl IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        eprintln!("{self}");
        (StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong").into_response()
    }
}
