macro_rules! stable_tokens {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($(#[$vmeta:meta])* $variant:ident => $token:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            strum::EnumIter,
        )]
        $vis enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            pub fn token(self) -> &'static str {
                match self { $($name::$variant => $token),+ }
            }

            pub fn from_token(token: &str) -> Option<Self> {
                match token { $($token => Some($name::$variant),)+ _ => None }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.token())
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.token())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let token = <std::borrow::Cow<'_, str> as serde::Deserialize>::deserialize(d)?;
                Self::from_token(&token).ok_or_else(|| {
                    serde::de::Error::unknown_variant(
                        &token,
                        &[$($token),+],
                    )
                })
            }
        }
    };
}

pub(super) use stable_tokens;
