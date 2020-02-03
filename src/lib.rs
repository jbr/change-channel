use delegate::delegate;
use std::ops::{Deref, DerefMut};
use std::sync::{
    mpsc, Arc, LockResult, Mutex, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
};

pub struct ChangeChannel<T: Sized + Clone> {
    rw_lock: Arc<RwLock<T>>,
    change_send: mpsc::Sender<ChangeChannelMessage<T>>,
    listeners: Arc<Mutex<Vec<Box<dyn Fn(&T) -> () + Send>>>>,
    pub thread_handle: Option<std::thread::JoinHandle<()>>,
}

unsafe impl<T: Sized + Send + Clone> Send for ChangeChannel<T> {}
unsafe impl<T: Sized + Send + Sync + Clone> Sync for ChangeChannel<T> {}

pub struct CCWriteLockGuard<'a, T: Clone + Sized + 'a> {
    send: mpsc::Sender<ChangeChannelMessage<T>>,
    rw_guard: RwLockWriteGuard<'a, T>,
}

impl<T: Sized + Clone> Drop for CCWriteLockGuard<'_, T> {
    fn drop(&mut self) {
        self.send
            .send(ChangeChannelMessage::Change(Arc::new(
                self.rw_guard.clone(),
            )))
            .ok();
        std::mem::drop(&self.rw_guard);
    }
}

impl<T: Clone> Deref for CCWriteLockGuard<'_, T> {
    type Target = T;
    delegate! {
        to self.rw_guard {
            fn deref(&self) -> &T;
        }
    }
}

impl<T: Clone> DerefMut for CCWriteLockGuard<'_, T> {
    delegate! {
        to self.rw_guard {
            fn deref_mut(&mut self) -> &mut T;
        }
    }
}

enum ChangeChannelMessage<T> {
    Shutdown,
    Change(Arc<T>),
}

impl<T: Clone + Send + Sync + 'static> ChangeChannel<T> {
    pub fn new(item: T) -> Self {
        let (change_send, change_receive) = mpsc::channel();
        let rw_lock = Arc::new(RwLock::new(item));
        let listeners: Arc<Mutex<Vec<Box<dyn Fn(&T) -> () + Send>>>> = Arc::new(Mutex::new(vec![]));

        let listeners_for_thread = listeners.clone();

        let thread_handle = std::thread::spawn(move || loop {
            match change_receive.recv() {
                Ok(ChangeChannelMessage::Shutdown) => break,
                Ok(ChangeChannelMessage::Change(new_value)) => {
                    for listener in &*listeners_for_thread.lock().unwrap() {
                        listener(&*new_value)
                    }
                }
                Err(_) => (),
            }
        });

        Self {
            rw_lock,
            change_send,
            listeners,
            thread_handle: Some(thread_handle),
        }
    }

    pub fn register<F: Fn(&T) -> () + Send + 'static>(&self, listener: F) {
        listener(&*self.read().unwrap());
        self.listeners.lock().unwrap().push(Box::new(listener));
    }

    pub fn write(&self) -> Result<CCWriteLockGuard<T>, PoisonError<RwLockWriteGuard<T>>> {
        Ok(CCWriteLockGuard {
            send: self.change_send.clone(),
            rw_guard: self.rw_lock.write()?,
        })
    }

    delegate! {
        to self.rw_lock {
            pub fn read(&self) -> LockResult<RwLockReadGuard<'_, T>>;
        }
    }
}

impl<T: Sized + Clone> Drop for ChangeChannel<T> {
    fn drop(&mut self) {
        self.change_send.send(ChangeChannelMessage::Shutdown).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn it_works() {
        let mut cc = ChangeChannel::new(0);
        let values_seen = Arc::new(Mutex::new(vec![]));

        let values_seen_for_thread = values_seen.clone();
        cc.register(move |value| values_seen_for_thread.lock().unwrap().push(*value));

        assert_eq!(*values_seen.lock().unwrap(), vec![0]);

        *cc.write().unwrap() = 20;
        assert_eq!(*cc.read().unwrap(), 20);

        *cc.write().unwrap() = 300;
        assert_eq!(*cc.read().unwrap(), 300);

        let thread_handle = cc.thread_handle.take();
        std::mem::drop(cc);
        thread_handle.unwrap().join().ok();
        assert_eq!(*values_seen.lock().unwrap(), vec![0, 20, 300]);
    }
}
