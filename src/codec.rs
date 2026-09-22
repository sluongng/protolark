//! Descriptor-driven protobuf conversion using protobuf JSON mapping.
use anyhow::{Context, Result, bail};
use prost::Message;
use prost_reflect::{
    DescriptorPool, DynamicMessage, MessageDescriptor, ReflectMessage, SerializeOptions, Value,
};
use serde_json::Value as Json;
use std::path::PathBuf;

fn descriptor(sets: &[PathBuf], name: &str) -> Result<MessageDescriptor> {
    if sets.is_empty() {
        bail!("at least one descriptor set is required");
    }
    // Merge all files before resolving imports, so set ordering is immaterial.
    let mut merged = prost_types::FileDescriptorSet { file: Vec::new() };
    for path in sets {
        let bytes = std::fs::read(path)
            .with_context(|| format!("read descriptor set {}", path.display()))?;
        let set = prost_types::FileDescriptorSet::decode(bytes.as_slice())
            .with_context(|| format!("parse descriptor set {}", path.display()))?;
        for mut file in set.file {
            file.source_code_info = None;
            if let Some(old) = merged.file.iter().find(|f| f.name == file.name) {
                if old != &file {
                    bail!("conflicting descriptors for {}", file.name());
                }
            } else {
                merged.file.push(file);
            }
        }
    }
    let pool = DescriptorPool::from_file_descriptor_set(merged)
        .context("resolve protobuf schema (descriptor sets must include imports)")?;
    pool.get_message_by_name(name.trim_start_matches('.'))
        .with_context(|| format!("message {name:?} not found in descriptor sets"))
}
fn from_json(sets: &[PathBuf], name: &str, value: &Json) -> Result<DynamicMessage> {
    let message = DynamicMessage::deserialize(descriptor(sets, name)?, value)
        .with_context(|| format!("invalid protobuf JSON for {name}"))?;
    validate_required(&message, name)?;
    Ok(message)
}
fn json(message: &DynamicMessage) -> Result<Json> {
    message
        .serialize_with_options(
            serde_json::value::Serializer,
            &SerializeOptions::new().use_proto_field_name(true),
        )
        .context("serialize protobuf JSON")
}
fn validate_required(message: &DynamicMessage, path: &str) -> Result<()> {
    for field in message.descriptor().fields() {
        if field.is_required() && !message.has_field(&field) {
            bail!("missing required protobuf field {path}.{}", field.name());
        }
    }
    for (field, value) in message.fields() {
        validate_value(value, &format!("{path}.{}", field.name()))?;
    }
    for (extension, value) in message.extensions() {
        validate_value(value, &format!("{path}.[{}]", extension.full_name()))?;
    }
    Ok(())
}
fn validate_value(value: &Value, path: &str) -> Result<()> {
    match value {
        Value::Message(message) => validate_required(message, path)?,
        Value::List(values) => {
            for (i, v) in values.iter().enumerate() {
                validate_value(v, &format!("{path}[{i}]"))?;
            }
        }
        Value::Map(values) => {
            for (k, v) in values {
                validate_value(v, &format!("{path}[{k:?}]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}
/// Encode protobuf JSON, including the standard well-known-type JSON mappings.
pub fn encode(sets: &[PathBuf], name: &str, value: &Json) -> Result<Vec<u8>> {
    Ok(normalized_json_message(sets, name, value)?.encode_to_vec())
}
/// Encode structural Starlark data. Well-known messages use their declared fields,
/// e.g. Timestamp is `{ "seconds": 0 }`, not an RFC3339 string.
pub fn encode_config(sets: &[PathBuf], name: &str, value: &Json) -> Result<Vec<u8>> {
    Ok(config_message(sets, name, value)?.encode_to_vec())
}
/// Decode binary to protobuf JSON (64-bit integers are strings, bytes are base64).
/// Unknown wire fields are accepted but omitted from JSON, as in protobuf libraries.
pub fn decode(sets: &[PathBuf], name: &str, bytes: &[u8]) -> Result<Json> {
    let message = DynamicMessage::decode(descriptor(sets, name)?, bytes)
        .with_context(|| format!("decode protobuf {name}"))?;
    validate_required(&message, name)?;
    json(&message)
}
pub fn encode_text(sets: &[PathBuf], name: &str, value: &Json) -> Result<String> {
    Ok(normalized_json_message(sets, name, value)?.to_text_format())
}
pub fn encode_config_text(sets: &[PathBuf], name: &str, value: &Json) -> Result<String> {
    Ok(config_message(sets, name, value)?.to_text_format())
}
pub fn decode_text(sets: &[PathBuf], name: &str, text: &str) -> Result<Json> {
    let message = DynamicMessage::parse_text_format(descriptor(sets, name)?, text)
        .with_context(|| format!("parse text protobuf {name}"))?;
    validate_required(&message, name)?;
    json(&message)
}

// prost-reflect stores maps in HashMaps, so its ordinary map serializer is not
// deterministic. Its ProtoJSON also treats google.protobuf.* specially, whereas
// Starlark constructors describe the actual schema fields. Use a private copy of
// the schema for serialization. Structural input additionally gets private type
// names to disable well-known JSON mappings, and JSON names equal to proto names.
// Ordinary ProtoJSON retains public type names, including text extension names.
// Field numbers, kinds, cardinalities, packing and defaults are untouched,
// so its wire format is identical. Map fields become repeated entry messages,
// whose order we explicitly sort. The protobuf library still does ALL wire and
// text serialization; no custom wire encoder is involved.
fn normalized_descriptor(
    original: &MessageDescriptor,
    structural: bool,
) -> Result<MessageDescriptor> {
    fn field(f: &mut prost_types::FieldDescriptorProto, structural: bool) {
        if !structural {
            return;
        }
        f.json_name = f.name.clone();
        for name in [&mut f.type_name, &mut f.extendee].into_iter().flatten() {
            *name = format!(".protolark_internal.{}", name.trim_start_matches('.'));
        }
    }
    fn message(m: &mut prost_types::DescriptorProto, structural: bool) {
        if let Some(options) = &mut m.options {
            options.map_entry = Some(false);
        }
        for f in &mut m.field {
            field(f, structural);
        }
        for f in &mut m.extension {
            field(f, structural);
        }
        for nested in &mut m.nested_type {
            message(nested, structural);
        }
    }
    let mut files: Vec<_> = original
        .parent_pool()
        .file_descriptor_protos()
        .cloned()
        .collect();
    for file in &mut files {
        if structural {
            file.package = Some(match file.package.as_deref() {
                None | Some("") => "protolark_internal".to_owned(),
                Some(package) => format!("protolark_internal.{package}"),
            });
        }
        for m in &mut file.message_type {
            message(m, structural);
        }
        for f in &mut file.extension {
            field(f, structural);
        }
        if !structural {
            continue;
        }
        for service in &mut file.service {
            for method in &mut service.method {
                for name in [&mut method.input_type, &mut method.output_type]
                    .into_iter()
                    .flatten()
                {
                    *name = format!(".protolark_internal.{}", name.trim_start_matches('.'));
                }
            }
        }
    }
    let pool =
        DescriptorPool::from_file_descriptor_set(prost_types::FileDescriptorSet { file: files })
            .context("normalize protobuf schema for deterministic serialization")?;
    let name = if structural {
        format!("protolark_internal.{}", original.full_name())
    } else {
        original.full_name().to_owned()
    };
    pool.get_message_by_name(&name)
        .context("normalized message is missing")
}

fn normalized_json_message(sets: &[PathBuf], name: &str, value: &Json) -> Result<DynamicMessage> {
    let mut original = from_json(sets, name, value)?;
    normalize_any_payloads(&mut original, 0)?;
    let desc = original.descriptor();
    let mut normalized = DynamicMessage::decode(
        normalized_descriptor(&desc, false)?,
        original.encode_to_vec().as_slice(),
    )?;
    sort_maps(&mut normalized, &desc);
    Ok(normalized)
}
// ProtoJSON's Any deserializer has already serialized its embedded message into
// opaque bytes. Normalize those payloads too, before the outer schema loses its
// WKT identity. This is intentionally used only by the ProtoJSON path: explicit
// `Any.value` bytes from structural configurations must remain byte-for-byte intact.
fn normalize_any_payloads(message: &mut DynamicMessage, depth: usize) -> Result<()> {
    if depth > 64 {
        bail!("embedded Any nesting exceeds 64 levels");
    }
    fn visit(value: &mut Value, depth: usize) -> Result<()> {
        match value {
            Value::Message(message) => normalize_any_payloads(message, depth)?,
            Value::List(values) => {
                for value in values {
                    visit(value, depth)?;
                }
            }
            Value::Map(values) => {
                for value in values.values_mut() {
                    visit(value, depth)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    for (_, value) in message.fields_mut() {
        visit(value, depth + 1)?;
    }
    for (_, value) in message.extensions_mut() {
        visit(value, depth + 1)?;
    }
    let descriptor = message.descriptor();
    if descriptor.full_name() != "google.protobuf.Any" {
        return Ok(());
    }
    let Some(type_url) = message.get_field_by_name("type_url") else {
        return Ok(());
    };
    let Some(type_url) = type_url.as_str() else {
        return Ok(());
    };
    let name = type_url.rsplit('/').next().unwrap_or(type_url);
    let Some(payload_descriptor) = descriptor.parent_pool().get_message_by_name(name) else {
        // Keep unknown payload types opaque, consistent with binary protobuf.
        return Ok(());
    };
    let Some(bytes) = message.get_field_by_name("value") else {
        return Ok(());
    };
    let Some(bytes) = bytes.as_bytes() else {
        return Ok(());
    };
    let mut payload = DynamicMessage::decode(payload_descriptor.clone(), bytes.as_ref())
        .with_context(|| format!("decode Any payload {name}"))?;
    validate_required(&payload, name)?;
    normalize_any_payloads(&mut payload, depth + 1)?;
    let mut normalized = DynamicMessage::decode(
        normalized_descriptor(&payload_descriptor, false)?,
        payload.encode_to_vec().as_slice(),
    )?;
    sort_maps(&mut normalized, &payload_descriptor);
    message.set_field_by_name("value", Value::Bytes(normalized.encode_to_vec().into()));
    Ok(())
}

fn config_message(sets: &[PathBuf], name: &str, value: &Json) -> Result<DynamicMessage> {
    let desc = descriptor(sets, name)?;
    let value = structural_json(value, &desc)?;
    let mut normalized =
        DynamicMessage::deserialize(normalized_descriptor(&desc, true)?, &value)
            .with_context(|| format!("invalid structural protobuf configuration for {name}"))?;
    validate_required(&normalized, name)?;
    sort_maps(&mut normalized, &desc);
    Ok(normalized)
}

fn structural_json(value: &Json, desc: &MessageDescriptor) -> Result<Json> {
    let Some(object) = value.as_object() else {
        return Ok(value.clone());
    };
    let mut result = serde_json::Map::new();
    for (name, value) in object {
        let field = desc
            .get_field_by_name(name)
            .or_else(|| desc.get_field_by_json_name(name))
            .with_context(|| format!("unknown protobuf field {}.{name}", desc.full_name()))?;
        let transformed = if value.is_null() {
            Json::Null
        } else if field.is_map() {
            let prost_reflect::Kind::Message(entry) = field.kind() else {
                unreachable!()
            };
            let key_field = entry.map_entry_key_field();
            let value_field = entry.map_entry_value_field();
            let values = value
                .as_object()
                .with_context(|| format!("{}.{} must be a map", desc.full_name(), name))?;
            let mut entries = Vec::new();
            for (key, value) in values {
                if value.is_null() {
                    bail!(
                        "protobuf map {}.{name}[{key:?}] cannot contain a null value",
                        desc.full_name()
                    );
                }
                let key = if key_field.kind() == prost_reflect::Kind::Bool {
                    Json::Bool(
                        key.parse()
                            .context("protobuf boolean map key must be true or false")?,
                    )
                } else {
                    Json::String(key.clone())
                };
                let value = if let prost_reflect::Kind::Message(child) = value_field.kind() {
                    structural_json(value, &child)?
                } else {
                    value.clone()
                };
                entries.push(serde_json::json!({ "key": key, "value": value }));
            }
            Json::Array(entries)
        } else if let prost_reflect::Kind::Message(child) = field.kind() {
            if field.is_list() {
                if let Some(values) = value.as_array() {
                    Json::Array(
                        values
                            .iter()
                            .map(|v| structural_json(v, &child))
                            .collect::<Result<_>>()?,
                    )
                } else {
                    value.clone()
                }
            } else {
                structural_json(value, &child)?
            }
        } else {
            value.clone()
        };
        if result
            .insert(field.name().to_owned(), transformed)
            .is_some()
        {
            bail!(
                "duplicate protobuf field {}.{}",
                desc.full_name(),
                field.name()
            );
        }
    }
    Ok(Json::Object(result))
}

fn sort_maps(message: &mut DynamicMessage, original: &MessageDescriptor) {
    fn sort_value(value: &mut Value, child: &MessageDescriptor, is_map: bool) {
        match value {
            Value::Message(message) => sort_maps(message, child),
            Value::List(values) => {
                for value in values.iter_mut() {
                    if let Value::Message(message) = value {
                        sort_maps(message, child);
                    }
                }
                if is_map {
                    values.sort_by_cached_key(|v| match v {
                        Value::Message(entry) => {
                            format!("{:?}", entry.get_field_by_name("key").unwrap())
                        }
                        _ => unreachable!("normalized map entries are messages"),
                    });
                }
            }
            _ => {}
        }
    }
    for (field, value) in message.fields_mut() {
        let Some(source_field) = original.get_field(field.number()) else {
            continue;
        };
        let prost_reflect::Kind::Message(child) = source_field.kind() else {
            continue;
        };
        sort_value(value, &child, source_field.is_map());
    }
    for (extension, value) in message.extensions_mut() {
        if let Some(source) = original.get_extension(extension.number())
            && let prost_reflect::Kind::Message(child) = source.kind()
        {
            sort_value(value, &child, false);
        }
    }
}
