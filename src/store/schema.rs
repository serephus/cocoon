//! Database schema.

diesel::table! {
    pastes (id) {
        id -> Text,
        owner -> Text,
        content -> Text,
        title -> Text,
        publish_at -> BigInt,
        created_at -> BigInt,
        updated_at -> BigInt,
        notified -> Bool,
    }
}

diesel::table! {
    conversations (user_id) {
        user_id -> BigInt,
        kind -> Text,
        step -> Text,
        paste_id -> Text,
        content -> Text,
        title -> Text,
        publish_at -> Nullable<BigInt>,
        updated_at -> BigInt,
    }
}

diesel::table! {
    subscriptions (user_id, paste_id) {
        user_id -> BigInt,
        paste_id -> Text,
        created_at -> BigInt,
    }
}

diesel::table! {
    updates (update_id) {
        update_id -> BigInt,
        received_at -> BigInt,
    }
}

diesel::allow_tables_to_appear_in_same_query!(pastes, conversations, subscriptions, updates);
