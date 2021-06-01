use crate::error::Error;
use base64::STANDARD;
use base64_serde::base64_serde_type;
use byteorder::{BigEndian, WriteBytesExt};
use serde::{Deserialize, Serialize};
use std::io::{Cursor, Write};
use std::{collections::BTreeMap, convert::TryFrom};

pub const PROTOCOL_VERSION: &'static str = "3.0.0";

base64_serde_type!(Base64Format, STANDARD);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Base64Buffer(#[serde(with = "Base64Format")] pub Vec<u8>);

impl From<Vec<u8>> for Base64Buffer {
    fn from(d: Vec<u8>) -> Self {
        Base64Buffer(d)
    }
}

impl ToString for Base64Buffer {
    fn to_string(&self) -> String {
        base64::encode(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    #[serde(rename = "request_id")]
    pub id: String,
    pub unix_seconds: i64,
    #[serde(rename = "a")]
    pub send_ack: bool,
    #[serde(rename = "v")]
    pub version: String,
    #[serde(flatten)]
    pub body: RequestBody,
}

impl Request {
    pub fn new(body: RequestBody) -> Self {
        Self {
            id: base64::encode(sodiumoxide::randombytes::randombytes(32)),
            send_ack: false,
            unix_seconds: chrono::Utc::now().timestamp(),
            version: PROTOCOL_VERSION.to_string(),
            body,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestBody {
    #[serde(rename = "me_request")]
    Id(IdRequest),

    #[serde(rename = "u2f_register_request")]
    Register(RegisterRequest),

    #[serde(rename = "u2f_authenticate_request")]
    Authenticate(AuthenticateRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdRequest {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub challenge: Base64Buffer,
    #[serde(rename = "app_id")]
    pub rp_id: String,
    pub rp_name: Option<String>,
    pub user: Option<UserData>,
    #[serde(rename = "webauthn")]
    pub is_webauthn: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserData {
    pub id: Base64Buffer,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticateRequest {
    pub challenge: Base64Buffer,
    #[serde(rename = "app_id")]
    pub rp_id: String,
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
    key_handle: Option<Base64Buffer>,
    key_handles: Option<Vec<Base64Buffer>>,
}

impl AuthenticateRequest {
    pub fn get_key_handles(&self) -> Vec<&Base64Buffer> {
        if let Some(kh) = &self.key_handles {
            kh.iter().collect()
        } else if let Some(kh) = &self.key_handle {
            vec![kh]
        } else {
            vec![]
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub request_id: String,

    #[serde(rename = "sns_endpoint_arn")]
    pub aws_push_id: Option<String>,
    pub device_token: Option<String>,

    #[serde(rename = "v")]
    pub version: String,
    #[serde(flatten)]
    pub body: ResponseBody,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ClientResult<T> {
    #[serde(flatten)]
    contents: Option<T>,
    error: Option<String>,
}

impl<T> Into<Result<T, Error>> for ClientResult<T> {
    fn into(self) -> Result<T, Error> {
        match (self.contents, self.error) {
            (Some(contents), None) => Ok(contents),
            (_, Some(e)) => Err(Error::DeviceError(e)),
            (_, _) => Err(Error::UnexpectedResponse),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseBody {
    #[serde(rename = "me_response")]
    Id(ClientResult<IdResponse>),

    #[serde(rename = "u2f_register_response")]
    Register(ClientResult<RegisterResponse>),

    #[serde(rename = "u2f_authenticate_response")]
    Authenticate(ClientResult<AuthenticateResponse>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdResponse {
    #[serde(rename = "me")]
    pub data: IdData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdData {
    pub email: String,
    pub device_identifier: Base64Buffer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub public_key: Base64Buffer,
    pub key_handle: Base64Buffer,
    pub attestation_certificate: Base64Buffer,
    pub signature: Base64Buffer,
    pub attestation_data: Base64Buffer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticateResponse {
    pub public_key: Base64Buffer,
    pub counter: u32,
    pub signature: Base64Buffer,
    pub key_handle: Base64Buffer,
    pub user_handle: Option<Base64Buffer>,
    pub authenticator_data: Base64Buffer,
}

impl TryFrom<ResponseBody> for IdResponse {
    type Error = crate::error::Error;

    fn try_from(value: ResponseBody) -> Result<Self, Error> {
        match value {
            ResponseBody::Id(resp) => resp.into(),
            _ => Err(Error::UnexpectedResponse),
        }
    }
}

impl TryFrom<ResponseBody> for RegisterResponse {
    type Error = crate::error::Error;

    fn try_from(value: ResponseBody) -> Result<Self, Error> {
        match value {
            ResponseBody::Register(resp) => resp.into(),
            _ => Err(Error::UnexpectedResponse),
        }
    }
}

impl TryFrom<ResponseBody> for AuthenticateResponse {
    type Error = crate::error::Error;

    fn try_from(value: ResponseBody) -> Result<Self, Error> {
        match value {
            ResponseBody::Authenticate(resp) => resp.into(),
            _ => Err(Error::UnexpectedResponse),
        }
    }
}

// Wire protocols
pub enum WireMessage {
    SealedMessage(Vec<u8>),
    SealedPublicKey(Vec<u8>),
}

impl WireMessage {
    pub fn new(data: Vec<u8>) -> Result<Self, Error> {
        let mut data = data.into_iter();

        match data.next() {
            Some(0x00) => Ok(WireMessage::SealedMessage(data.collect())),
            Some(0x02) => Ok(WireMessage::SealedPublicKey(data.collect())),
            _ => Err(Error::InvalidWireProtocol),
        }
    }

    pub fn into_wire(self) -> Vec<u8> {
        match self {
            Self::SealedMessage(data) => vec![vec![0x00], data].concat(),
            Self::SealedPublicKey(data) => vec![vec![0x02], data].concat(),
        }
    }
}