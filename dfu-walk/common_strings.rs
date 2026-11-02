//! Unity's built-in "common strings" table. Type-tree node names/types whose string offset has the
//! high bit set index into this blob instead of the file-local string buffer.
//! Offsets are stable across Unity 5.x-2021.x (entries are only ever appended).

pub const COMMON_STRINGS: &[u8] = b"AABB\0AnimationClip\0AnimationCurve\0AnimationState\0Array\0Base\0BitField\0bitset\0bool\0char\0ColorRGBA\0Component\0data\0deque\0double\0dynamic_array\0FastPropertyName\0first\0float\0Font\0GameObject\0Generic Mono\0GradientNEW\0GUID\0GUIStyle\0int\0list\0long long\0map\0Matrix4x4f\0MdFour\0MonoBehaviour\0MonoScript\0m_ByteSize\0m_Curve\0m_EditorClassIdentifier\0m_EditorHideFlags\0m_Enabled\0m_ExtensionPtr\0m_GameObject\0m_Index\0m_IsArray\0m_IsStatic\0m_MetaFlag\0m_Name\0m_ObjectHideFlags\0m_PrefabInternal\0m_PrefabParentObject\0m_Script\0m_StaticEditorFlags\0m_Type\0m_Version\0Object\0pair\0PPtr<Component>\0PPtr<GameObject>\0PPtr<Material>\0PPtr<MonoBehaviour>\0PPtr<MonoScript>\0PPtr<Object>\0PPtr<Prefab>\0PPtr<Sprite>\0PPtr<TextAsset>\0PPtr<Texture>\0PPtr<Texture2D>\0PPtr<Transform>\0Prefab\0Quaternionf\0Rectf\0RectInt\0RectOffset\0second\0set\0short\0size\0SInt16\0SInt32\0SInt64\0SInt8\0staticvector\0string\0TextAsset\0TextMesh\0Texture\0Texture2D\0Transform\0TypelessData\0UInt16\0UInt32\0UInt64\0UInt8\0unsigned int\0unsigned_long long\0unsigned short\0vector\0Vector2f\0Vector3f\0Vector4f\0m_ScriptingClassIdentifier\0Gradient\0Type*\0int2_storage\0int3_storage\0BoundsInt\0m_CorrespondingSourceObject\0m_PrefabInstance\0m_PrefabAsset\0FileSize\0Hash128\0RenderingLayerMask\0";

/// Resolves a NUL-terminated string at `offset` inside `table`.
pub fn lookup(table: &[u8], offset: usize) -> Option<&str> {
    let rest = table.get(offset..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    std::str::from_utf8(&rest[..end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_offsets() {
        // Offsets cross-checked against the type trees of a real Unity 2019.4 bundle.
        assert_eq!(lookup(COMMON_STRINGS, 0), Some("AABB"));
        assert_eq!(lookup(COMMON_STRINGS, 49), Some("Array"));
        assert_eq!(lookup(COMMON_STRINGS, 427), Some("m_Name"));
        assert_eq!(lookup(COMMON_STRINGS, 874), Some("Texture2D"));
        assert_eq!(lookup(COMMON_STRINGS, 894), Some("TypelessData"));
        assert_eq!(lookup(COMMON_STRINGS, 1161), Some("Hash128"));
    }
}
