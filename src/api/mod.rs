//! A unified API for abstracting over various exchanges.

pub mod binance;
pub mod gdax;
pub mod errors;
mod params;
mod wss;

use crate::*;
use order_book::LimitUpdate;
use futures::prelude::*;
use std::ops::Deref;

pub use self::params::*;

pub type Timestamp = u64;

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Deserialize, Serialize)]
/// Wrapper around a type carrying an additionnal timestamp. Deref to `T`.
pub struct Timestamped<T> {
    timestamp: Timestamp,
    #[serde(flatten)]
    inner: T,
}

impl<T> Timestamped<T> {
    /// Registered timestamp.
    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Return the wrapped value.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T> Deref for Timestamped<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

trait IntoTimestamped: Sized {
    fn timestamped(self) -> Timestamped<Self> {
        Timestamped {
            timestamp: timestamp_ms(),
            inner: self,
        }
    }

    fn with_timestamp(self, timestamp: Timestamp) -> Timestamped<Self> {
        Timestamped {
            timestamp,
            inner: self,
        }
    }
}

impl<T: Sized> IntoTimestamped for T { }

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
/// See https://www.investopedia.com/terms/t/timeinforce.asp.
pub enum TimeInForce {
    GoodTilCanceled,
    ImmediateOrCancel,
    FillOrKilll,
}

trait AsStr {
    fn as_str(&self) -> &'static str;
}

impl AsStr for Side {
    fn as_str(&self) -> &'static str {
        match self {
            Side::Ask => "SELL",
            Side::Bid => "BUY",
        }
    }
}

impl AsStr for TimeInForce {
    fn as_str(&self) -> &'static str {
        match self {
            TimeInForce::GoodTilCanceled => "GTC",
            TimeInForce::FillOrKilll => "FOK",
            TimeInForce::ImmediateOrCancel => "IOC",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
/// An order to be sent through the API.
pub struct Order {
    price: Price,
    size: Size,
    side: Side,
    time_in_force: TimeInForce,
    time_window: u64,
    order_id: Option<String>,
}

impl Order {
    /// Return a new `Order`, with:
    /// * `price` being the order price
    /// * `size` being the order size
    /// * `side` being `Side::Bid` (buy) or `Side::Ask` (sell)
    pub fn new(price: Price, size: Size, side: Side) -> Self {
        Order {
            price,
            size,
            side,
            time_in_force: TimeInForce::GoodTilCanceled,
            time_window: 5000,
            order_id: None,
        }
    }

    /// Time in force, see https://www.investopedia.com/terms/t/timeinforce.asp.
    pub fn time_in_force(mut self, time_in_force: TimeInForce) -> Self {
        self.time_in_force = time_in_force;
        self
    }

    /// Delay until the order becomes invalid if not treated by the server, in ms.
    /// Unusable on gdax, the exchange forces the time window to be 30s.
    pub fn time_window(mut self, time_window: u64) -> Self {
        self.time_window = time_window;
        self
    }

    /// Unique id used to identify this order. Automatically generated if not specified.
    pub fn order_id(mut self, order_id: String) -> Self {
        self.order_id = Some(order_id);
        self
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
/// An order to cancel a previous order.
pub struct Cancel {
    order_id: String,
    time_window: u64,
}

impl Cancel {
    /// Return a new `Cancel`, with `order_id` identifying the order to cancel.
    pub fn new(order_id: String) -> Self {
        Cancel {
            order_id,
            time_window: 5000,
        }
    }

    /// Delay until the cancel order becomes invalid if not treated by the server, in ms.
    /// Unusable on gdax, the exchange forces the time window to be 30s.
    pub fn time_window(mut self, time_window: u64) -> Self {
        self.time_window = time_window;
        self
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
/// An acknowledgment that an order has been treated by the server.
pub struct OrderAck {
    /// ID identifiying the order.
    pub order_id: String,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
/// An acknowledgment that a cancel order has been treated by the server.
pub struct CancelAck {
    /// ID identifying the canceled order.
    pub order_id: String,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
/// A notification that some order has been updated, i.e. a trade crossed through this order.
pub struct OrderUpdate {
    /// ID identifying the order being updated.
    pub order_id: String,

    /// Size just consumed by last trade.
    pub consumed_size: Size,

    /// Total remaining size for this order (can be maintained in a standalone way
    /// using the size of the order at insertion time, `consumed_size` and `commission`).
    pub remaining_size: Size,

    /// Price at which the last trade happened.
    pub consumed_price: Price,

    /// Commission amount (warning: for binance this may not be in the same currency as
    /// the traded asset).
    pub commission: Size,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
/// A liquidity consuming order.
pub struct Trade {
    /// Price in ticks.
    pub price: Price,

    /// Size consumed by the trade.
    pub size: Size,

    /// Side of the maker:
    /// * if `Ask`, then the maker was providing liquidity on the ask side,
    ///   i.e. the consumer bought to the maker
    /// * if `Bid`, then the maker was providing liquidity on the bid side,
    ///   i.e. the consumer sold to the maker
    pub maker_side: Side,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
/// A notification that some order has expired or was canceled.
pub struct OrderExpiration {
    /// Expired order.
    pub order_id: String,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
/// A notification that some order has been received by the exchange.
pub struct OrderConfirmation {
    /// Unique order id.
    pub order_id: String,

    /// Price at which the order was inserted.
    pub price: Price,

    /// Size at which the order was inserted.
    pub size: Price,

    /// Side of the order.
    pub side: Side,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
/// A notification that some event happened.
pub enum Notification {
    /// A trade was executed.
    Trade(Timestamped<Trade>),

    /// The limit order book has changed and should be updated.
    LimitUpdates(Vec<Timestamped<LimitUpdate>>),

    OrderConfirmation(Timestamped<OrderConfirmation>),

    /// An order has been updated.
    OrderUpdate(Timestamped<OrderUpdate>),

    /// An order has expired or was canceled.
    OrderExpiration(Timestamped<OrderExpiration>),
}

/// A trait implemented by clients of various exchanges API.
pub trait ApiClient {
    /// Type returned by the `stream` implementor, used for continuously receiving
    /// notifications.
    type Stream: Stream<Item = Notification, Error = ()> + Send + 'static;

    /// Start streaming notifications.
    fn stream(&self) -> Self::Stream;

    /// Send an order to the exchange.
    fn order(&self, order: &Order)
        -> Box<Future<Item = Timestamped<OrderAck>, Error = errors::OrderError> + Send + 'static>;

    /// Send a cancel order to the exchange.
    fn cancel(&self, cancel: &Cancel)
        -> Box<Future<Item = Timestamped<CancelAck>, Error = errors::CancelError> + Send + 'static>;

    /// Send a ping to the exchange.
    fn ping(&self)
        -> Box<Future<Item = Timestamped<()>, Error = errors::Error> + Send + 'static>;

    /// Generate an order id. When possible, the result will be equal to `hint`, otherwise
    /// it is assured that all strings generated by a call to this method are distinct.
    fn new_order_id(hint: &str) -> String;
}
