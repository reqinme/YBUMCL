//! Local version inheritance; all metadata paths remain under the canonical game root.
use super::*;

const MAX_INHERITANCE_DEPTH: usize = 32;

pub(super) struct ResolvedVersion {
    pub metadata: Value,
    pub client_id: String,
}

pub(super) fn resolve_version(root: &Path, id: &str) -> Result<ResolvedVersion> {
    resolve(root, id, &mut BTreeSet::new())
}

fn resolve(root: &Path, id: &str, visiting: &mut BTreeSet<String>) -> Result<ResolvedVersion> {
    if !safe_component(id) {
        return Err(CoreError::Metadata(format!(
            "unsafe inherited version ID: {id}"
        )));
    }
    if visiting.contains(id) {
        return Err(CoreError::Metadata(format!(
            "version inheritance cycle at {id}"
        )));
    }
    if visiting.len() >= MAX_INHERITANCE_DEPTH {
        return Err(CoreError::Metadata(format!(
            "version inheritance exceeds {MAX_INHERITANCE_DEPTH} metadata files"
        )));
    }
    visiting.insert(id.into());
    let path = root.join("versions").join(id).join(format!("{id}.json"));
    inside(root, &path)?;
    let mut child = read_json(&path)?;
    if !child.is_object() || child["id"].as_str() != Some(id) {
        return Err(CoreError::Metadata(format!(
            "Version metadata ID does not match the selected directory: {id}"
        )));
    }
    // Validate each layer before merging so a child cannot conceal unsupported configuration.
    if child.get("mainClass").is_some() {
        if let Some(reason) = unsupported(&child, id) {
            return Err(CoreError::Unsupported(reason));
        }
    } else if let Some(reason) = unsupported_configuration(&child, id) {
        return Err(CoreError::Unsupported(reason));
    }
    normalize_arguments(&mut child)?;
    if let Some(libraries) = child.get("libraries") {
        for library in libraries
            .as_array()
            .ok_or_else(|| CoreError::Metadata("libraries must be an array".into()))?
        {
            library_key(library)?;
        }
    }
    let parent_id = child
        .get("inheritsFrom")
        .map(|parent| {
            parent.as_str().map(str::to_owned).ok_or_else(|| {
                CoreError::Metadata("inheritsFrom must be a version ID string".into())
            })
        })
        .transpose()?;
    let resolved = if let Some(parent_id) = parent_id {
        let mut parent = resolve(root, &parent_id, visiting)?;
        merge(&mut parent.metadata, child)?;
        parent
    } else {
        ResolvedVersion {
            metadata: child,
            client_id: id.into(),
        }
    };
    visiting.remove(id);
    Ok(resolved)
}

fn normalize_arguments(metadata: &mut Value) -> Result<()> {
    if let Some(arguments) = metadata.get("arguments") {
        let arguments = arguments
            .as_object()
            .ok_or_else(|| CoreError::Metadata("arguments must be an object".into()))?;
        for name in ["game", "jvm"] {
            if arguments.get(name).is_some_and(|value| !value.is_array()) {
                return Err(CoreError::Metadata(format!(
                    "arguments.{name} must be an array"
                )));
            }
        }
    }
    // Normalize legacy arguments before merging, including a legacy parent with a modern child.
    if let Some(legacy) = metadata.get("minecraftArguments") {
        let legacy = legacy
            .as_str()
            .ok_or_else(|| CoreError::Metadata("minecraftArguments must be a string".into()))?;
        let game = split_legacy(legacy)?;
        if metadata.get("arguments").is_none() {
            metadata["arguments"] = serde_json::json!({});
        }
        match metadata["arguments"]["game"].as_array_mut() {
            Some(existing) => {
                // 有的启动器（PCL 的 Forge+LiteLoader+OptiFine 合并实例）同时写了两份：
                // minecraftArguments 带 --username/--accessToken 等通用参数，arguments.game
                // 只有加载器的 tweakClass。丢掉前者会让游戏因为缺 accessToken 直接退出。
                let mut merged: Vec<Value> = game.into_iter().map(Value::String).collect();
                merged.append(existing);
                *existing = merged;
            }
            None => metadata["arguments"]["game"] = serde_json::json!(game),
        }
    }
    Ok(())
}

fn library_key(library: &Value) -> Result<(String, String, String)> {
    let name = library["name"]
        .as_str()
        .ok_or_else(|| CoreError::Metadata("library name is missing".into()))?;
    // Reuse the launcher's coordinate and path validation even for overridden libraries.
    safe_relative(&maven_path(name)?)?;
    let parts: Vec<_> = name.split(':').collect();
    Ok((
        parts[0].into(),
        parts[1].into(),
        parts.get(3).copied().unwrap_or("").into(),
    ))
}

fn merge(parent: &mut Value, child: Value) -> Result<()> {
    let parent = parent.as_object_mut().unwrap();
    for (key, value) in child.as_object().unwrap() {
        match key.as_str() {
            "inheritsFrom" => {}
            "arguments" => {
                let arguments = parent
                    .entry(key.clone())
                    .or_insert_with(|| serde_json::json!({}));
                for (kind, values) in value.as_object().unwrap() {
                    if matches!(kind.as_str(), "game" | "jvm") {
                        arguments
                            .as_object_mut()
                            .unwrap()
                            .entry(kind.clone())
                            .or_insert_with(|| serde_json::json!([]))
                            .as_array_mut()
                            .unwrap()
                            .extend(values.as_array().unwrap().iter().cloned());
                    } else {
                        arguments[kind] = values.clone();
                    }
                }
            }
            "libraries" => {
                let libraries = parent
                    .entry(key.clone())
                    .or_insert_with(|| serde_json::json!([]))
                    .as_array_mut()
                    .unwrap();
                for library in value.as_array().unwrap() {
                    let key = library_key(library)?;
                    // Remove every older occurrence; classifier artifacts remain distinct.
                    libraries.retain(|old| library_key(old).is_ok_and(|old_key| old_key != key));
                    libraries.push(library.clone());
                }
            }
            _ => {
                parent.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(())
}
