//! Results without waiting, and what the library knows of the server
//! (docs/offline.md, "Reachability and results without waiting"): the
//! two-step search and account check, and the reachability watch for the
//! app and the keyboard. All are `stickpile_core`'s
//! (`Session::search_steps`, `Session::account_steps`, `Session::reach`);
//! this hands them over to Kotlin.

use std::sync::{Arc, Mutex};

use stickpile_core::{AccountStep, ServerStep, Session};

use crate::library::{falls_back, lock, AbortOnDrop, Inner, StickpileLibrary};
use crate::types::*;
use crate::LibraryError;

impl Inner {
    /// Who is signed in (`StickpileLibrary::account`), and the server's
    /// answer to come when that is the cached account
    /// (`Session::account_steps`).
    pub(crate) async fn account_answer(
        &self,
        session: &Session,
    ) -> Result<(Option<Account>, Option<AccountStep>), LibraryError> {
        let server_url = session.config().server_url.clone();
        let auth = if session.is_logged_in() {
            AuthKind::Token
        } else {
            AuthKind::Tailnet
        };
        match session.account_steps().await {
            Ok(steps) => Ok((Some(Account::new(&steps.first, &server_url, auth)), steps.then)),
            Err(e) if e.is_unauthorized() => Ok((None, None)),
            // Nothing cached and no server to ask: without a token that is
            // signed out, whatever the server would say about a tailnet
            // identity; with one, not known.
            Err(e) if falls_back(&e) && !session.is_logged_in() => Ok((None, None)),
            Err(e) => Err(e.into()),
        }
    }
}

/// The server's answer to "who is signed in" after the cached account came
/// first (`AccountAnswer.pending`). The request runs on its own: nobody
/// waiting for it does not cancel it.
#[derive(uniffi::Object)]
pub struct PendingAccount {
    /// Taken by the first `server_account`.
    step: Mutex<Option<AccountStep>>,
    handle: tokio::runtime::Handle,
    /// The cached account's server and sign-in, for the account the
    /// server answers with.
    server_url: String,
    auth: AuthKind,
}

impl std::fmt::Debug for PendingAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingAccount")
            .field("waiting", &lock(&self.step).is_some())
            .finish()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PendingAccount {
    /// Wait for the server's word on the cached account: the account (the
    /// token still works), or `None` when the server refused the token (a
    /// revoked token: signed out, as `account` says). Throws `Unreachable`
    /// when the server could not be reached or failed (the cached account
    /// stands), and once it was answered (one answer per check); other
    /// errors as `account`. Stops waiting when the calling coroutine is
    /// cancelled; the request goes on.
    pub async fn server_account(&self) -> Result<Option<Account>, LibraryError> {
        let Some(step) = lock(&self.step).take() else {
            return Err(LibraryError::Unreachable {
                detail: "the server's answer was handed over already".into(),
            });
        };
        let (server_url, auth) = (self.server_url.clone(), self.auth);
        let task = self.handle.spawn(async move {
            match step.wait().await {
                Ok(Some(user)) => Ok(Some(Account::new(&user, &server_url, auth))),
                Ok(None) => Err(LibraryError::Unreachable {
                    detail: "the server did not say who is signed in; the cached account stands".into(),
                }),
                Err(e) if e.is_unauthorized() => Ok(None),
                Err(e) => Err(e.into()),
            }
        });
        let _abort = AbortOnDrop(task.abort_handle());
        task.await?
    }
}

/// The server's answer to a search whose offline results came first
/// (`SearchAnswer.pending`). The request runs on its own: nobody waiting
/// for it does not cancel it, so that its outcome still tells the library
/// whether the server is there.
#[derive(uniffi::Object)]
pub struct PendingSearch {
    /// Taken by the first `server_results`.
    step: Mutex<Option<ServerStep>>,
    inner: Arc<Inner>,
    handle: tokio::runtime::Handle,
    /// The session and generation the search ran in, as `search` keeps
    /// what it read ([`Inner::list`]).
    session: Arc<Session>,
    generation: u64,
}

impl std::fmt::Debug for PendingSearch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingSearch")
            .field("waiting", &lock(&self.step).is_some())
            .finish()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PendingSearch {
    /// Wait for the server's results, which replace the offline ones in
    /// place (`offline = false`). `None` when the server could not be
    /// reached or failed: the offline results stand (and after a failure
    /// to reach it the library believes it unreachable, `server_reach`).
    /// Throws what the server answered instead, such as `Unauthorized`
    /// (the offline results stand then too). Once: a second call answers
    /// `None`. Stops waiting when the calling coroutine is cancelled; the
    /// request goes on.
    pub async fn server_results(&self) -> Result<Option<ItemList>, LibraryError> {
        let Some(step) = lock(&self.step).take() else {
            return Ok(None);
        };
        let (inner, session, generation) = (self.inner.clone(), self.session.clone(), self.generation);
        let task = self.handle.spawn(async move {
            let Some(out) = step.wait().await? else {
                return Ok(None);
            };
            let items = out.hits.into_iter().map(|h| h.item).collect();
            Ok(Some(inner.list(generation, &session, items, out.offline)))
        });
        let _abort = AbortOnDrop(task.abort_handle());
        task.await?
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl StickpileLibrary {
    /// Search as `search` does, without waiting for a server that does not
    /// answer (`Session::search_steps`). With the server believed
    /// unreachable the offline cache answers at once and nothing is asked.
    /// Else the server is asked: its results if they come within 300 ms
    /// (or the offline cache's at once when it fails); if not, the offline
    /// cache's now (`offline = true`) and the server's to wait for
    /// (`SearchAnswer.pending`). Throws as `search`.
    pub async fn search_steps(&self, query: String, target: Target, limit: u32) -> Result<SearchAnswer, LibraryError> {
        let handle = self.handle.clone();
        self.read(move |inner| async move {
            let (generation, session) = inner.current();
            let query = inner.search_query(&session, query).await?;
            let steps = session
                .search_steps(&query, Some(target.into()), limit as usize)
                .await?;
            let offline = steps.first.offline;
            let items = steps.first.hits.into_iter().map(|h| h.item).collect();
            let list = inner.list(generation, &session, items, offline);
            let pending = steps.then.map(|step| {
                Arc::new(PendingSearch {
                    step: Mutex::new(Some(step)),
                    inner: inner.clone(),
                    handle,
                    session,
                    generation,
                })
            });
            Ok(SearchAnswer { list, pending })
        })
        .await
    }

    /// Who is signed in, as `account` answers, and when that is the cached
    /// account because the server had not answered within 300 ms, the
    /// server's word on it to come (`AccountAnswer.pending`): for the app's
    /// start, which shows the library at once and signs out when the server
    /// then refuses the token. Throws as `account`.
    pub async fn account_steps(&self) -> Result<AccountAnswer, LibraryError> {
        let handle = self.handle.clone();
        self.read(move |inner| async move {
            let session = inner.session();
            let (account, step) = inner.account_answer(&session).await?;
            let pending = match (step, &account) {
                (Some(step), Some(cached)) => Some(Arc::new(PendingAccount {
                    step: Mutex::new(Some(step)),
                    handle,
                    server_url: cached.server_url.clone(),
                    auth: cached.auth,
                })),
                _ => None,
            };
            Ok(AccountAnswer { account, pending })
        })
        .await
    }

    /// What the library knows of the server now. No network.
    pub fn server_reach(&self) -> ServerReach {
        self.inner.session().client().reach_now().into()
    }

    /// What the library knows of the server once it differs from `seen`
    /// (at once when it does already): for the app and the keyboard, which
    /// run the search again and sync when it turns `Reachable` after
    /// `Unreachable`. Follows the new server's session after the server
    /// changed. Stops waiting when the calling coroutine is cancelled.
    pub async fn server_reach_change(&self, seen: ServerReach) -> Result<ServerReach, LibraryError> {
        self.read(move |inner| async move {
            let mut sessions = inner.sessions.subscribe();
            loop {
                let mut reach = inner.session().reach();
                let changed = async {
                    reach
                        .wait_for(|now| ServerReach::from(*now) != seen)
                        .await
                        .map(|now| ServerReach::from(*now))
                };
                tokio::select! {
                    now = changed => match now {
                        Ok(now) => return Ok(now),
                        // The session is gone, so another one took its place.
                        Err(_) => {
                            let _ = sessions.changed().await;
                        }
                    },
                    _ = sessions.changed() => {}
                }
            }
        })
        .await
    }

    /// Check the server now, in the background (`Session::check_server`):
    /// when the app or the keyboard opens and when a network appears. What
    /// it finds comes through `server_reach_change`. Returns at once;
    /// callable from any thread. No network on the calling thread.
    pub fn check_server(&self) {
        self.inner.session().check_server();
    }
}
