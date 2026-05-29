use chrono::{Datelike, NaiveDateTime, Timelike};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sqlx::types::{
    time::{Date, PrimitiveDateTime, Time},
    Uuid,
};
use time::Month;
use uuid::Uuid as UuidStd;

#[derive(Clone, Debug)]
pub struct SerializableUuid(pub Uuid);

impl std::fmt::Display for SerializableUuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.to_string())
    }
}

impl Default for SerializableUuid {
    fn default() -> Self {
        SerializableUuid(Uuid::nil())
    }
}

impl Serialize for SerializableUuid {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for SerializableUuid {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let uuid = UuidStd::parse_str(&s).map_err(serde::de::Error::custom)?;
        Ok(SerializableUuid(Uuid::from(uuid)))
    }
}

impl From<Uuid> for SerializableUuid {
    fn from(uuid: Uuid) -> Self {
        SerializableUuid(uuid)
    }
}

impl From<Option<Uuid>> for SerializableUuid {
    fn from(uuid: Option<Uuid>) -> Self {
        match uuid {
            Some(uuid) => SerializableUuid(uuid),
            None => SerializableUuid(Uuid::nil()),
        }
    }
}

impl From<PrimitiveDateTime> for SerializableDateTime {
    fn from(dt: PrimitiveDateTime) -> Self {
        SerializableDateTime(dt)
    }
}

#[derive(Clone, Debug)]
pub struct SerializableDateTime(pub PrimitiveDateTime);

impl Default for SerializableDateTime {
    fn default() -> Self {
        SerializableDateTime(PrimitiveDateTime::new(
            Date::from_calendar_date(1970, Month::January, 1).unwrap(),
            Time::from_hms(0, 0, 0).unwrap(),
        ))
    }
}

impl Serialize for SerializableDateTime {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let timestamp = self.0.assume_utc().unix_timestamp();
        serializer.serialize_i64(timestamp)
    }
}

impl<'de> Deserialize<'de> for SerializableDateTime {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let timestamp = i64::deserialize(deserializer)?;
        let naive_datetime = NaiveDateTime::from_timestamp(timestamp, 0);

        let year = naive_datetime.year();
        let month = Month::try_from(naive_datetime.month() as u8).unwrap();
        let day = naive_datetime.day() as u8;
        let hour = naive_datetime.hour() as u8;
        let minute = naive_datetime.minute() as u8;
        let second = naive_datetime.second() as u8;

        let date = Date::from_calendar_date(year, month, day).unwrap();
        let time = Time::from_hms(hour, minute, second).unwrap();
        let dt = PrimitiveDateTime::new(date, time);
        Ok(SerializableDateTime(dt))
    }
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct Simulation {
    pub id: SerializableUuid,
    pub project_id: i32,
    pub chain_id: String,
    pub block_at: i32,
    pub transaction_version: i32,
    pub nonce: i32,
    pub max_fee: String,
    pub cairo_version: String,
    pub wallet_address: String,
    pub calldata: Option<Vec<String>>,
    pub created_at: SerializableDateTime,
    pub updated_at: SerializableDateTime,
    pub status: String,
    pub error_message: Option<String>,
    pub error_contract_address: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: i32,
    pub name: String,
    pub slug: String,
}

#[derive(Clone, Debug)]
pub struct User {
    pub email: String,
}
