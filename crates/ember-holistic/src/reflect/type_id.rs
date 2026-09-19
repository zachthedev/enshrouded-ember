//! Type ids.
//!
//! A type id is `FNV-1a` 32 of the qualified name, so code computes it rather
//! than looking it up. That word is the low half of the 64-bit id a descriptor
//! record carries, and it is the half every lookup keys on.
//!
//! The high half is not a name hash. It repeats across `keen::ecs::X` and
//! `keen::ds::ecs::X`, and across unrelated types that share a layout, so it
//! identifies a shape rather than a type.

/// The `FNV-1a` 32 offset basis.
const BASIS: u32 = 0x811c_9dc5;

/// The `FNV-1a` 32 prime.
const PRIME: u32 = 0x0100_0193;

/// `FNV-1a` 32 over `bytes`, continuing from `hash`.
///
/// The hash is a fold over bytes, so a qualified name is hashed as its prefix
/// followed by its bare name. That is what lets `component` and `data_store`
/// stay `const` without building a string.
const fn fold(hash: u32, bytes: &[u8]) -> u32 {
    let mut acc: u32 = hash;
    let mut index: usize = 0;
    while index < bytes.len() {
        acc ^= bytes[index] as u32;
        acc = acc.wrapping_mul(PRIME);
        index += 1;
    }
    acc
}

/// A reflected type, identified by the hash of its qualified name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(
    /// `FNV-1a` 32 of the fully qualified name.
    pub u32,
);

impl TypeId {
    /// `FNV-1a` 32 of the fully qualified C++ name, which is what the engine
    /// stores.
    #[must_use]
    pub const fn from_qualified_name(name: &str) -> Self {
        Self(fold(BASIS, name.as_bytes()))
    }

    /// The in-memory component type, `keen::ecs::<name>`.
    #[must_use]
    pub const fn component(name: &str) -> Self {
        Self(fold(fold(BASIS, b"keen::ecs::"), name.as_bytes()))
    }

    /// The data-store type the serializer uses, `keen::ds::ecs::<name>`.
    #[must_use]
    pub const fn data_store(name: &str) -> Self {
        Self(fold(fold(BASIS, b"keen::ds::ecs::"), name.as_bytes()))
    }
}

/// The `TypeId` of a component, from its bare name.
///
/// The argument is any `const` expression of type `&str`, because every
/// component name in this project lives as a const in one module and reaches
/// this macro through an accessor rather than as a literal. The expansion is a
/// `const` expression, so it is usable wherever a constant is.
///
/// # Examples
///
/// ```
/// use ember_holistic::{component_id, reflect::type_id::TypeId};
///
/// const NAME: &str = "Inventory";
/// const fn bare() -> &'static str {
///     "Inventory"
/// }
///
/// const FROM_LITERAL: TypeId = component_id!("Inventory");
/// const FROM_CONST: TypeId = component_id!(NAME);
/// const FROM_CALL: TypeId = component_id!(bare());
///
/// assert_eq!(FROM_LITERAL, TypeId(0xb195_28a9));
/// assert_eq!(FROM_CONST, FROM_LITERAL);
/// assert_eq!(FROM_CALL, FROM_LITERAL);
/// ```
#[macro_export]
macro_rules! component_id {
    ($name:expr) => {
        $crate::reflect::type_id::TypeId::component($name)
    };
}

#[cfg(test)]
mod tests {
    use super::TypeId;

    /// The ids the descriptor table carries for five known component types.
    ///
    /// Each expected value is read off the server's own table, so the test
    /// judges the hash rather than recording what this code returns.
    #[test]
    fn component_ids_match_the_descriptor_table() {
        let cases: [(&str, u32); 5] = [
            ("InventoryCraftingStock", 0xe41a_0d1f),
            ("Inventory", 0xb195_28a9),
            ("Crafting", 0xad77_6575),
            ("BaseIdComponent", 0x4927_f4e2),
            ("OwnerRelationship", 0xd736_1788),
        ];

        for (name, expected) in cases {
            assert_eq!(
                TypeId::component(name),
                TypeId(expected),
                "component {name}"
            );
        }
    }

    /// A data-store id hashes a different prefix over the same bare name.
    #[test]
    fn a_data_store_id_is_not_its_component_id() {
        assert_eq!(
            TypeId::data_store("InventoryCraftingStock"),
            TypeId(0xa9cc_aafc)
        );
        assert_ne!(
            TypeId::data_store("InventoryCraftingStock"),
            TypeId::component("InventoryCraftingStock")
        );
    }

    /// The two prefixes are the ones the engine spells.
    ///
    /// This pins the prefix strings and nothing else. Both sides run the same
    /// fold over the same basis and prime, so a wrong basis or a wrong prime
    /// leaves it green.
    #[test]
    fn a_qualified_name_hashes_to_the_same_id_as_its_prefix_and_bare_name() {
        let cases: [(&str, TypeId); 2] = [
            ("keen::ecs::Inventory", TypeId::component("Inventory")),
            ("keen::ds::ecs::Inventory", TypeId::data_store("Inventory")),
        ];

        for (qualified, expected) in cases {
            assert_eq!(
                TypeId::from_qualified_name(qualified),
                expected,
                "{qualified}"
            );
        }
    }

    /// The offset basis is the published `FNV-1a` 32 constant.
    ///
    /// Hashing the empty name folds nothing, so this is the one assertion in
    /// the file that checks a constant against its published value rather than
    /// against another path through the same code.
    #[test]
    fn the_empty_name_hashes_to_the_published_offset_basis() {
        assert_eq!(TypeId::from_qualified_name(""), TypeId(0x811c_9dc5));
    }

    /// A prefix folds into the bare name rather than being hashed apart from
    /// it.
    #[test]
    fn a_prefix_folds_into_the_name_that_follows_it() {
        assert_eq!(
            TypeId::component(""),
            TypeId::from_qualified_name("keen::ecs::")
        );
        assert_ne!(TypeId::component("Inventory"), TypeId::component(""));
    }

    /// The macro takes a const expression, not only a literal.
    ///
    /// Every component name reaches it through a const accessor, so a
    /// `literal` fragment would make the macro unusable by its one consumer.
    #[test]
    fn the_macro_takes_a_const_expression_in_a_const_context() {
        const NAME: &str = "Inventory";
        const fn bare() -> &'static str {
            "Inventory"
        }

        const FROM_LITERAL: TypeId = component_id!("Inventory");
        const FROM_CONST: TypeId = component_id!(NAME);
        const FROM_CALL: TypeId = component_id!(bare());

        assert_eq!(FROM_LITERAL, TypeId(0xb195_28a9));
        assert_eq!(FROM_CONST, TypeId(0xb195_28a9));
        assert_eq!(FROM_CALL, TypeId(0xb195_28a9));
    }
}
