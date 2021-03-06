use futures::sync::mpsc::{unbounded, UnboundedReceiver};
use std::thread;
use std::collections::HashMap;
use chashmap::CHashMap;
use std::sync::Arc;
use log::{debug, error};
use failure::bail;
use serde_derive::{Serialize, Deserialize};
use crate::{tick, Side};
use crate::order_book::LimitUpdate;
use crate::api::{
    Notification,
    NotificationFlags,
    OrderConfirmation,
    OrderUpdate,
    Trade,
    OrderExpiration,
};
use crate::api::symbol::Symbol;
use crate::api::wss;
use crate::api::timestamp::{convert_str_timestamp, timestamp_ms, IntoTimestamped};
use crate::api::gdax::{Keys, Client};

impl Client {
    crate fn new_stream(&self, symbol: Symbol, flags: NotificationFlags)
        -> UnboundedReceiver<Notification>
    {
        let streaming_endpoint = self.params.streaming_endpoint.clone();
        let keys = self.keys.clone();
        let order_ids = self.order_ids.clone();
        let (snd, rcv) = unbounded();
        thread::spawn(move || {
            debug!("initiating WebSocket connection at {}", streaming_endpoint);
            
            if let Err(err) = ws::connect(streaming_endpoint, |out| {
                wss::Handler::new(out, snd.clone(), wss::KeepAlive::False, HandlerImpl {
                    symbol,
                    flags,
                    state: SubscriptionState::NotSubscribed,
                    keys: keys.clone(),
                    orders: HashMap::new(),
                    order_ids: order_ids.clone(),
                })
            })
            {
                error!("WebSocket connection terminated with error: `{}`", err);
            }
        });
        
        rcv
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
enum SubscriptionState {
    NotSubscribed,
    Subscribed,
}

struct HandlerImpl {
    symbol: Symbol,
    flags: NotificationFlags,
    state: SubscriptionState,
    keys: Option<Keys>,

    /// server order id => client order
    orders: HashMap<String, OrderConfirmation>,

    /// client order id => server order id (shared with `Client`)
    order_ids: Arc<CHashMap<String, String>>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize)]
#[serde(untagged)]
enum GdaxChannel<'a> {
    Channel(&'a str),
    WithProducts {
        name: &'a str,
        product_ids: &'a [&'a str],
    },
}

#[derive(Clone, Debug, Serialize)]
struct GdaxAuth<'a> {
    key: &'a str,
    signature: String,
    timestamp: f64,
    passphrase: &'a str,
}

#[derive(Clone, Debug, Serialize)]
struct GdaxSubscription<'a> {
    #[serde(rename = "type")]
    type_: &'a str,
    product_ids: &'a [&'a str],
    channels: Vec<GdaxChannel<'a>>,

    #[serde(flatten)]
    auth: Option<GdaxAuth<'a>>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Deserialize)]
struct GdaxBookSnapshot<'a> {
    #[serde(borrow)]
    bids: Vec<(&'a str, &'a str)>,
    #[serde(borrow)]
    asks: Vec<(&'a str, &'a str)>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Deserialize)]
struct GdaxLimitUpdate<'a> {
    #[serde(borrow)]
    changes: Vec<(&'a str, &'a str, &'a str)>,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Deserialize)]
struct GdaxMatch<'a> {
    time: &'a str,
    size: &'a str,
    price: &'a str,
    side: &'a str,
    maker_order_id: &'a str,
    taker_order_id: &'a str,
    profile_id: Option<&'a str>,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Deserialize)]
struct GdaxReceived<'a> {
    time: &'a str,
    client_oid: Option<&'a str>,
    order_id: &'a str,
    size: &'a str,
    price: &'a str,
    side: &'a str,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Deserialize)]
struct GdaxDone<'a> {
    reason: &'a str,
    order_id: &'a str,
    time: &'a str,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Deserialize)]
struct GdaxError<'a> {
    message: &'a str,
    reason: Option<&'a str>,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Deserialize)]
struct EventType<'a> {
    #[serde(rename = "type")]
    type_: &'a str,
}

impl HandlerImpl {
    fn convert_gdax_update(&self, l: (&str, &str), side: Side)
        -> Result<LimitUpdate, tick::ConversionError>
    {
        Ok(
            LimitUpdate {
                side,
                price: self.symbol.price_tick().ticked(l.0)?,
                size: self.symbol.size_tick().ticked(l.1)?,
            }
        )
    }

    fn convert_gdax_side(&self, side: &str) -> Result<Side, failure::Error> {
        let side = match side {
            "buy" => Side::Bid,
            "sell" => Side::Ask,
            other => bail!("wrong side: `{}`", other),
        };
        Ok(side)
    }

    fn parse_message(&mut self, json: &str, out: &wss::NotifSender) -> Result<(), failure::Error> {
        let event_type: EventType<'_> = serde_json::from_str(json)?;

        match event_type.type_ {
            "subscribe" => {
                if self.state != SubscriptionState::NotSubscribed {
                    error!("received `subscribe` event while already subscribed");
                }
                self.state = SubscriptionState::Subscribed;
            },

            "snapshot" if self.flags.contains(NotificationFlags::ORDER_BOOK) => {
                let snapshot: GdaxBookSnapshot<'_> = serde_json::from_str(json)?;

                let bid = snapshot.bids
                    .into_iter()
                    .map(|(price, size)| self.convert_gdax_update((price, size), Side::Bid))
                    .map(|l| Ok(l?.timestamped()));

                let ask = snapshot.asks
                    .into_iter()
                    .map(|(price, size)| self.convert_gdax_update((price, size), Side::Ask))
                    .map(|l| Ok(l?.timestamped()));
                
                let notif = Notification::LimitUpdates(
                    bid.chain(ask).collect::<Result<Vec<_>, tick::ConversionError>>()?
                );
                out.unbounded_send(notif).unwrap();
            },

            "l2update" if self.flags.contains(NotificationFlags::ORDER_BOOK) => {
                let update: GdaxLimitUpdate<'_> = serde_json::from_str(json)?;

                let updates = update.changes
                    .into_iter()
                    .map(|(side, price, size)| {
                        let side = self.convert_gdax_side(side)?;
                        Ok(self.convert_gdax_update((price, size), side)?)
                    })
                    .map(|l: Result<_, failure::Error>| Ok(l?.timestamped()));

                let updates = updates.collect::<Result<Vec<_>, failure::Error>>()?;
                if !updates.is_empty() {
                    let notif = Notification::LimitUpdates(updates);
                    out.unbounded_send(notif).unwrap();
                }
            },

            "match"
                if self.flags.contains(NotificationFlags::TRADES | NotificationFlags::ORDERS) =>
            {
                let trade: GdaxMatch<'_> = serde_json::from_str(json)?;
                let timestamp = convert_str_timestamp(trade.time)?;
                
                let size = self.symbol.size_tick().ticked(trade.size)?;
                let price = self.symbol.price_tick().ticked(trade.price)?;

                // An order which is about us
                if self.flags.contains(NotificationFlags::ORDERS) && trade.profile_id.is_some() {
                    let update_order = |order: &mut OrderConfirmation| {
                        order.size -= size;

                        out.unbounded_send(
                            Notification::OrderUpdate(OrderUpdate {
                                order_id: order.order_id.clone(),
                                consumed_size: size,
                                consumed_price: price,
                                remaining_size: order.size,
                                commission: 0,
                            }.with_timestamp(timestamp))
                        ).unwrap();
                    };

                    // These two conditions are exclusive.
                    if let Some(order) = self.orders.get_mut(trade.taker_order_id) {
                        update_order(order);
                    }
                    if let Some(order) = self.orders.get_mut(trade.maker_order_id) {
                        update_order(order);
                    }
                }

                if self.flags.contains(NotificationFlags::TRADES) {
                    out.unbounded_send(
                        Notification::Trade(Trade {
                            size,
                            price,
                            maker_side: self.convert_gdax_side(trade.side)?,
                        }.with_timestamp(timestamp))
                    ).unwrap();
                }
            },

            "received" if self.flags.contains(NotificationFlags::ORDERS) => {
                let received: GdaxReceived<'_> = serde_json::from_str(json)?;
                let timestamp = convert_str_timestamp(received.time)?;

                let size = self.symbol.size_tick().ticked(received.size)?;
                let price = self.symbol.price_tick().ticked(received.price)?;
                let side = self.convert_gdax_side(received.side)?;

                // The order id specified by the user, which defaults to the server order id
                // in case it was left unspecified.
                let order_id = received.client_oid.map(|oid| oid.to_owned())
                    .unwrap_or_else(|| received.order_id.to_owned());
                
                // Don't forget to update the concurrent map `server order id => client order id`
                // in case the WebSocket notif arrives before the HTTP response
                self.order_ids.insert(order_id.clone(), received.order_id.to_owned());
                debug!("insert order id {} (from WSS)", order_id);
                
                let order = OrderConfirmation {
                    size,
                    price,
                    side,
                    order_id,
                };

                self.orders.insert(received.order_id.to_owned(), order.clone());

                out.unbounded_send(
                    Notification::OrderConfirmation(order.with_timestamp(timestamp))
                ).unwrap();
            }

            "done" if self.flags.contains(NotificationFlags::ORDERS) => {
                let done: GdaxDone<'_> = serde_json::from_str(json)?;
                let timestamp = convert_str_timestamp(done.time)?;

                if done.reason != "canceled" {
                    return Ok(());
                }

                let order_id = match self.orders.get(done.order_id) {
                    Some(order) => order.order_id.to_owned(),
                    None => return Ok(()),
                };

                out.unbounded_send(
                    Notification::OrderExpiration(OrderExpiration {
                        order_id,
                    }.with_timestamp(timestamp))
                ).unwrap();
            }

            "error" => {
                let error: GdaxError<'_> = serde_json::from_str(json)?;
                bail!("{}: {:?}", error.message, error.reason);
            }

            _ => (),
        };
        Ok(())
    }
}

impl wss::HandlerImpl for HandlerImpl {
    fn on_open(&mut self, out: &ws::Sender) -> ws::Result<()> {
        let product_ids = [self.symbol.name()];
        let mut channels = vec![
            GdaxChannel::Channel("level2"),
            GdaxChannel::Channel("matches"),
            GdaxChannel::WithProducts {
                name: "heartbeat",
                product_ids: &product_ids,
            },
        ];

        let auth = self.keys.as_ref().map(|keys| {
            use openssl::{sign::Signer, hash::MessageDigest};

            let timestamp = timestamp_ms() as f64 / 1000.;
            let mut signer = Signer::new(MessageDigest::sha256(), &keys.secret_key).unwrap();
            let what = format!("{}GET/users/self/verify", timestamp);
            signer.update(what.as_bytes()).unwrap();
            let signature = base64::encode(&signer.sign_to_vec().unwrap());

            channels.push(GdaxChannel::Channel("user"));
            GdaxAuth {
                key: &keys.api_key,
                signature,
                timestamp,
                passphrase: &keys.pass_phrase,
            }
        });

        let subscription = GdaxSubscription {
            type_: "subscribe",
            product_ids: &product_ids,
            channels,
            auth,
        };
        
        match serde_json::to_string(&subscription) {
            Ok(value) => out.send(value),
            Err(err) => {
                panic!("failed to serialize `Subscription`: `{}`", err);
            }
        }
    }

    fn on_message(&mut self, text: &str, out: &wss::NotifSender) -> Result<(), failure::Error> {
        self.parse_message(text, out)
    }
}
