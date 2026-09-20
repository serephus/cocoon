//! Database schema.

diesel::table! {
    pastes (id) {
        id -> Binary,
        title -> Text,
        content -> Binary,
        publish_at -> BigInt,
        created_at -> BigInt,
    }
}
