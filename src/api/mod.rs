pub mod binance;

use crate::*;
use futures::prelude::*;

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
/// See https://www.investopedia.com/terms/t/timeinforce.asp.
pub enum TimeInForce {
    GoodTilCanceled,
    ImmediateOrCancel,
    FillOrKilll,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
/// An order to be sent through the API.
pub struct Order {
    /// Order side: `Bid` / buy or `Ask`/ sell.
    pub side: Side,

    /// Order price, stringified.
    pub price: String,

    /// Order size, stringified.
    pub size: String,

    /// Time in force, see https://www.investopedia.com/terms/t/timeinforce.asp.
    pub time_in_force: TimeInForce,

    /// Delay until the order becomes invalid if not treated by the server, in ms.
    pub time_window: u64,

    /// Unique id used to identify this order, stringified.
    /// Automatically generated if `None`.
    pub order_id: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
/// An order to cancel a previous order.
pub struct Cancel {
    /// Identify the order to be canceled.
    pub order_id: String,

    /// Delay until the cancel order becomes invalid if not treated by the server, in ms.
    pub time_window: u64,

    /// Unique id used to identify this cancel order, stringified.
    /// Automatically generated if `None`.
    pub cancel_id: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
/// An acknowledgment that an order has been treated by the server.
pub struct OrderAck {
    /// ID identifiying the order.
    pub order_id: String,

    /// Timestamp at which the order was treated, in ms.
    pub timestamp: u64,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
/// An acknowledgment that a cancel order has been treated by the server.
pub struct CancelAck {
    /// ID identifying the cancel order.
    pub cancel_id: String,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
/// A notification that some order has been updated, i.e. a trade crossed through this order.
pub struct OrderUpdate {
    /// ID identifying the order being updated.
    pub order_id: String,

    /// Size just consumed by last trade.
    pub consumed_size: Size,

    /// Total remaining size for this order (can be maintained in a standalone way
    /// using `consumed_size` and `commission`).
    pub total_size: Size,

    /// Price at which the last trade happened.
    pub consumed_price: Price,

    /// Commission amount (warning: for binance this may not be in the same currency as
    /// the traded asset).
    pub commission: Size,

    /// Timestamp at which the update happened, in ms.
    pub timestamp: u64,
}

#[derive(Clone, PartialEq, Eq, Debug)]
/// A notification that some event happened.
pub enum Notification {
    /// A trade was executed.
    Trade(Trade),

    /// The limit order book has changed and should be updated.
    LimitUpdates(Vec<LimitUpdate>),

    /// An order has been updated.
    OrderUpdate(OrderUpdate),

    /// An order has expired.
    OrderExpired(String),
}

/// A trait implemented by clients of various exchanges API.
pub trait ApiClient {
    /// Type returned by the `stream` implementor, used for continuously receiving
    /// notifications.
    type Stream: Stream<Item = Notification, Error = ()>;

    /// Type returned by the `order` implementor, used for awaiting the execution of
    /// an order.
    type FutureOrder: Future<Item = OrderAck, Error = Error>;

    /// Type returned by the `cancel` implementor, used for awaiting the execution of
    /// a cancel order.
    type FutureCancel: Future<Item = CancelAck, Error = Error>;

    /// Type returned by the `ping` implementor, used for awaiting the execution of
    /// a ping.
    type FuturePing: Future<Item = (), Error = Error>;

    /// Start streaming notifications.
    fn stream(&self) -> Self::Stream;

    /// Send an order to the exchange.
    fn order(&self, order: Order) -> Self::FutureOrder;

    /// Send a cancel order to the exchange.
    fn cancel(&self, cancel: Cancel) -> Self::FutureCancel;

    /// Send a ping to the exchange.
    fn ping(&self) -> Self::FuturePing;
}
