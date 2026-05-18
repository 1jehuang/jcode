use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::time::{Instant, SystemTime};

#[derive(Debug, Clone, Copy)]
pub struct InstantWrapper(Instant);

impl InstantWrapper {
    pub fn now() -> Self {
        Self(Instant::now())
    }
    
    pub fn into_inner(self) -> Instant {
        self.0
    }
    
    pub fn as_instant(&self) -> &Instant {
        &self.0
    }
}

impl Default for InstantWrapper {
    fn default() -> Self {
        Self::now()
    }
}

impl Serialize for InstantWrapper {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let duration = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        duration.as_nanos().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for InstantWrapper {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self::now())
    }
}
