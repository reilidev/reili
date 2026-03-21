#[derive(Debug, Clone, PartialEq)]
pub enum LogFieldValue {
    String(String),
    I64(i64),
    U64(u64),
    Bool(bool),
    F64(f64),
}

impl LogFieldValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::I64(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::U64(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::F64(value) => Some(*value),
            _ => None,
        }
    }
}

impl From<String> for LogFieldValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for LogFieldValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<bool> for LogFieldValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

macro_rules! impl_from_signed {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl From<$ty> for LogFieldValue {
                fn from(value: $ty) -> Self {
                    Self::I64(value as i64)
                }
            }
        )+
    };
}

macro_rules! impl_from_unsigned {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl From<$ty> for LogFieldValue {
                fn from(value: $ty) -> Self {
                    Self::U64(value as u64)
                }
            }
        )+
    };
}

impl_from_signed!(i8, i16, i32, i64);
impl_from_unsigned!(u8, u16, u32, u64);

impl From<isize> for LogFieldValue {
    fn from(value: isize) -> Self {
        match i64::try_from(value) {
            Ok(value) => Self::I64(value),
            Err(_) => Self::String(value.to_string()),
        }
    }
}

impl From<usize> for LogFieldValue {
    fn from(value: usize) -> Self {
        match u64::try_from(value) {
            Ok(value) => Self::U64(value),
            Err(_) => Self::String(value.to_string()),
        }
    }
}

impl From<i128> for LogFieldValue {
    fn from(value: i128) -> Self {
        match i64::try_from(value) {
            Ok(value) => Self::I64(value),
            Err(_) => Self::String(value.to_string()),
        }
    }
}

impl From<u128> for LogFieldValue {
    fn from(value: u128) -> Self {
        match u64::try_from(value) {
            Ok(value) => Self::U64(value),
            Err(_) => Self::String(value.to_string()),
        }
    }
}

impl From<f64> for LogFieldValue {
    fn from(value: f64) -> Self {
        Self::F64(value)
    }
}

impl From<f32> for LogFieldValue {
    fn from(value: f32) -> Self {
        Self::F64(f64::from(value))
    }
}
