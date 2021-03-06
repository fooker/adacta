use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::str::FromStr;

use anyhow::{anyhow, Error};
use base58::{FromBase58, ToBase58};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, Serializer};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct DocId(Uuid);

impl DocId {
    pub fn random() -> Self { Self(Uuid::new_v4()) }

    pub fn to_base58(&self) -> String { self.0.as_bytes().to_base58() }
}

impl FromStr for DocId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let id = s
            .from_base58()
            .map_err(|_| anyhow!("Invalid document ID"))?;
        Ok(Self(Uuid::from_slice(&id)?))
    }
}

impl Serialize for DocId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where S: Serializer {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for DocId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where D: serde::de::Deserializer<'de> {
        struct DocIdVisitor;

        impl<'de> serde::de::Visitor<'de> for DocIdVisitor {
            type Value = DocId;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("A base64 encoded document ID")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Self::Value::from_str(value).map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_str(DocIdVisitor)
    }
}

impl std::fmt::Display for DocId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_base58())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Document,
    Preview,
    Plaintext,
    Metadata,
    Other { name: OsString },
}

impl Kind {
    pub fn other(name: impl Into<OsString>) -> Self { Self::Other { name: name.into() } }
}

impl<S: Into<String>> From<S> for Kind {
    fn from(fragment: S) -> Self {
        let fragment = fragment.into();
        return match fragment.as_str() {
            "document" => Kind::Document,
            "preview" => Kind::Preview,
            "plaintext" => Kind::Plaintext,
            "metadata" => Kind::Metadata,
            s => Kind::other(s),
        };
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq, Deserialize, Serialize)]
pub struct Label(String);

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<S: Into<String>> From<S> for Label {
    fn from(s: S) -> Self { Self(s.into()) }
}

impl Borrow<String> for Label {
    fn borrow(&self) -> &String { &self.0 }
}

impl Borrow<str> for Label {
    fn borrow(&self) -> &str { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct Metadata {
    pub uploaded: DateTime<Utc>,
    pub archived: Option<DateTime<Utc>>,

    pub title: Option<String>,
    pub pages: u32,

    pub labels: HashSet<Label>,

    pub properties: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocInfo {
    pub id: DocId,
    pub metadata: Metadata,
}

impl<D, M> From<(D, M)> for DocInfo
    where D: Into<DocId>,
          M: Into<Metadata> {
    fn from((id, metadata): (D, M)) -> Self {
        return Self {
            id: id.into(),
            metadata: metadata.into(),
        };
    }
}