use core::borrow::Borrow;
use core::fmt::{self, Display};
use core::hash::Hash;
use core::iter;
use core::ops::Deref;

use std::collections::HashMap;

use tokio::sync::oneshot;

#[derive(Debug, Eq, PartialEq)]
pub enum Error {
    UnknownTopic,
    Duplicate,
    Deadlock,
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnknownTopic => write!(f, "unknown topic"),
            Error::Duplicate => write!(f, "duplicate"),
            Error::Deadlock => write!(f, "deadlock"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Debug)]
pub struct Sender<K, V>(HashMap<K, Option<oneshot::Sender<V>>>);

#[derive(Debug)]
pub struct Receiver<K, V>(HashMap<K, Option<oneshot::Receiver<V>>>);

pub fn channel<K, V>(topics: impl IntoIterator<Item = K>) -> (Sender<K, V>, Receiver<K, V>)
where
    K: Hash + Eq + Clone,
{
    let (tx, rx) = iter::zip(topics, iter::repeat_with(oneshot::channel))
        .map(|(topic, (tx, rx))| ((topic.clone(), Some(tx)), (topic, Some(rx))))
        .unzip();
    (Sender(tx), Receiver(rx))
}

impl<K, V> Deref for Sender<K, V> {
    type Target = HashMap<K, Option<oneshot::Sender<V>>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<K, V> Deref for Receiver<K, V> {
    type Target = HashMap<K, Option<oneshot::Receiver<V>>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<K, V> Sender<K, V>
where
    K: Hash + Eq,
{
    pub fn send(&mut self, topic: impl Borrow<K>, value: V) -> Result<(), Error> {
        let tx = self.0.get_mut(topic.borrow()).ok_or(Error::UnknownTopic)?;
        let tx = tx.take().ok_or(Error::Duplicate)?;
        tx.send(value).or(Err(Error::Deadlock))
    }
}

impl<K, V> Receiver<K, V>
where
    K: Hash + Eq,
{
    pub fn take(&mut self, topic: impl Borrow<K>) -> Result<oneshot::Receiver<V>, Error> {
        let rx = self.0.get_mut(topic.borrow()).ok_or(Error::UnknownTopic)?;
        let rx = rx.take().ok_or(Error::Duplicate)?;
        Ok(rx)
    }
}
