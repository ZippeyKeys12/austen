use serde::Deserialize;

pub fn from_str<'de, T: Deserialize<'de>>(json: &'de str) -> Result<T, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_str(json);
    deserializer.disable_recursion_limit();
    let value = T::deserialize(serde_stacker::Deserializer::new(&mut deserializer))?;
    deserializer.end()?;
    Ok(value)
}
