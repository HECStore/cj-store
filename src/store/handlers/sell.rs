//! `sell` / `s` command: validate input and enqueue a sell order.

use tracing::{debug, warn};

use super::super::{Store, utils};
use super::validation::{validate_item_name, validate_quantity};
use crate::messages::QueuedOrderType;

pub(super) async fn handle(
    store: &mut Store,
    player_name: &str,
    user_uuid: &str,
    parts: &[&str],
    command: &str,
) -> Result<(), String> {
    if parts.len() < 3 {
        warn!("Invalid sell command format from {}: {}", player_name, command);
        return utils::send_message_to_player(
            store,
            player_name,
            "Usage: sell <item> <quantity>. Example: sell cobblestone 64",
        )
        .await;
    }

    if let Err(e) = validate_item_name(parts[1]) {
        return utils::send_message_to_player(store, player_name, &e).await;
    }
    let item = utils::normalize_item_id(parts[1]);

    let quantity = match validate_quantity(parts[2], "sell") {
        Ok(q) => q,
        Err(e) => {
            warn!("Invalid quantity provided by {}: {}", player_name, parts[2]);
            return utils::send_message_to_player(store, player_name, &e).await;
        }
    };

    if !store.pairs.contains_key(&item) {
        return utils::send_message_to_player(
            store,
            player_name,
            &format!("Item '{}' is not available for trading", item),
        )
        .await;
    }

    debug!(
        "Queueing sell order: {} wants to sell {} of {}",
        player_name, quantity, item
    );

    match store.order_queue.add(
        user_uuid.to_string(),
        player_name.to_string(),
        QueuedOrderType::Sell,
        item.clone(),
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
