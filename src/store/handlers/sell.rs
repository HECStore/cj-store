//! `sell` / `s` command: enqueue a sell order.
//!
//! Input validation (item name, quantity) happens in `store::command::parse_command`.
//! This handler only checks runtime preconditions (is the pair tradable?) and
//! enqueues the order.

use tracing::debug;

use super::super::{Store, utils};
use crate::error::StoreError;
use crate::messages::QueuedOrderType;

pub(super) async fn handle(
    store: &mut Store,
    player_name: &str,
    user_uuid: &str,
    item: &str,
    quantity: u32,
) -> Result<(), StoreError> {
    if !store.pairs.contains_key(item) {
        debug!(
            player = player_name,
            uuid = user_uuid,
            item = item,
            "Rejected sell command: item not tradable"
        );
        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Item '{}' is not available for trading", item),
        )
        .await;
    }

    debug!(
        player = player_name,
        uuid = user_uuid,
        item = item,
        quantity = quantity,
        "Queueing sell order"
    );

    match store.order_queue.add(
        user_uuid.to_string(),
        player_name.to_string(),
        QueuedOrderType::Sell,
        item.to_string(),
        quantity,
    ) {
        Ok((order_id, position)) => {
            let queue_len = store.order_queue.len();
            let wait_estimate = store.order_queue.estimate_wait(position);
            let msg = format!(
                "Order #{} queued (position {}/{}). Est. wait: {}. You have {} order(s) pending.",
                order_id,
                position,
                queue_len,
                wait_estimate,
                store.order_queue.user_order_count(user_uuid)
            );
            utils::send_message_to_player(store, player_name, &msg).await
        }
        Err(e) => utils::send_message_to_player(store, player_name, &e).await,
    }
}
