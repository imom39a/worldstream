//! Deadline-bound transport for publication SQL. Mutation clients stay synchronous.
use std::{
    future::Future,
    net::{IpAddr, ToSocketAddrs},
    sync::{Arc, OnceLock},
    time::Instant,
};

use postgres::{Client, GenericClient, Row, Transaction, types::ToSql};
use tokio::{runtime::Runtime, sync::Semaphore, task::JoinHandle};
use tokio_postgres::{
    Config,
    config::{Host, LoadBalanceHosts},
};

const RESOLVERS: usize = 8;

// This is the pinned driver's own timeout error constructor, also used by
// postgres::Client. Keep the same operational error surface as synchronous SQL.
fn unavailable() -> postgres::Error {
    postgres::Error::__private_api_timeout()
}

fn runtime() -> Result<&'static Runtime, postgres::Error> {
    static RUNTIME: OnceLock<Result<Runtime, std::io::Error>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .max_blocking_threads(RESOLVERS)
                .thread_name("worldstream-publication-read")
                .enable_all()
                .build()
        })
        .as_ref()
        .map_err(|_| unavailable())
}

fn wait<T>(
    runtime: &Runtime,
    deadline: Instant,
    future: impl Future<Output = Result<T, postgres::Error>>,
) -> Result<T, postgres::Error> {
    if Instant::now() >= deadline {
        return Err(unavailable());
    }
    runtime.block_on(async {
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), future)
            .await
            .map_err(|_| unavailable())?
    })
}

/// Minimal query surface used by the existing authority, prefix, and page readers.
pub(super) trait ReadQuery {
    fn query(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, postgres::Error>;
    fn query_one(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Row, postgres::Error>;
    fn query_opt(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Option<Row>, postgres::Error>;
    fn batch_execute(&mut self, sql: &str) -> Result<(), postgres::Error>;
}
impl<C: GenericClient> ReadQuery for C {
    fn query(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, postgres::Error> {
        GenericClient::query(self, sql, params)
    }
    fn query_one(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Row, postgres::Error> {
        GenericClient::query_one(self, sql, params)
    }
    fn query_opt(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Option<Row>, postgres::Error> {
        GenericClient::query_opt(self, sql, params)
    }
    fn batch_execute(&mut self, sql: &str) -> Result<(), postgres::Error> {
        GenericClient::batch_execute(self, sql)
    }
}

pub(super) enum ReadConnection {
    Sync(Client),
    Bounded(BoundedClient),
}
impl ReadConnection {
    pub(super) fn connect(
        config: &super::PostgresConnectionConfig,
    ) -> Result<Self, postgres::Error> {
        match worldstream_core::storage_read_deadline() {
            Some(deadline) => BoundedClient::connect(config, deadline).map(Self::Bounded),
            None => Client::connect(&config.dsn, config.tls.clone()).map(Self::Sync),
        }
    }
    pub(super) fn transaction(&mut self) -> Result<ReadTransaction<'_>, postgres::Error> {
        match self {
            Self::Sync(client) => client.transaction().map(ReadTransaction::Sync),
            Self::Bounded(client) => {
                // FOR SHARE is part of the existing authority cut and cannot
                // run in PostgreSQL's READ ONLY transaction mode.
                client.batch_execute("BEGIN")?;
                Ok(ReadTransaction::Bounded(client))
            }
        }
    }
}

pub(super) enum ReadTransaction<'a> {
    Sync(Transaction<'a>),
    Bounded(&'a mut BoundedClient),
}
impl ReadTransaction<'_> {
    pub(super) fn commit(self) -> Result<(), postgres::Error> {
        match self {
            Self::Sync(tx) => tx.commit(),
            Self::Bounded(client) => client.batch_execute("COMMIT"),
        }
    }
    pub(super) fn query_one(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Row, postgres::Error> {
        ReadQuery::query_one(self, sql, params)
    }
    pub(super) fn query_opt(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Option<Row>, postgres::Error> {
        ReadQuery::query_opt(self, sql, params)
    }
    pub(super) fn batch_execute(&mut self, sql: &str) -> Result<(), postgres::Error> {
        ReadQuery::batch_execute(self, sql)
    }
}
impl ReadQuery for ReadTransaction<'_> {
    fn query(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, postgres::Error> {
        match self {
            Self::Sync(tx) => tx.query(sql, params),
            Self::Bounded(client) => client.query(sql, params),
        }
    }
    fn query_one(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Row, postgres::Error> {
        match self {
            Self::Sync(tx) => tx.query_one(sql, params),
            Self::Bounded(client) => client.query_one(sql, params),
        }
    }
    fn query_opt(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Option<Row>, postgres::Error> {
        match self {
            Self::Sync(tx) => tx.query_opt(sql, params),
            Self::Bounded(client) => client.query_opt(sql, params),
        }
    }
    fn batch_execute(&mut self, sql: &str) -> Result<(), postgres::Error> {
        match self {
            Self::Sync(tx) => tx.batch_execute(sql),
            Self::Bounded(client) => client.batch_execute(sql),
        }
    }
}

pub(super) struct BoundedClient {
    runtime: &'static Runtime,
    deadline: Instant,
    client: Option<tokio_postgres::Client>,
    connection: Option<JoinHandle<Result<(), postgres::Error>>>,
}
impl BoundedClient {
    fn connect(
        config: &super::PostgresConnectionConfig,
        deadline: Instant,
    ) -> Result<Self, postgres::Error> {
        if Instant::now() >= deadline {
            return Err(unavailable());
        }
        let runtime = runtime()?;
        let source: Config = config.dsn.parse()?;
        let (client, connection) = wait(
            runtime,
            deadline,
            connect_candidates(source, config.tls.clone()),
        )?;
        let connection = runtime.spawn(connection);
        Ok(Self {
            runtime,
            deadline,
            client: Some(client),
            connection: Some(connection),
        })
    }
    fn client(&self) -> Result<&tokio_postgres::Client, postgres::Error> {
        self.client.as_ref().ok_or_else(unavailable)
    }
}
impl ReadQuery for BoundedClient {
    fn query(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, postgres::Error> {
        wait(
            self.runtime,
            self.deadline,
            self.client()?.query(sql, params),
        )
    }
    fn query_one(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Row, postgres::Error> {
        wait(
            self.runtime,
            self.deadline,
            self.client()?.query_one(sql, params),
        )
    }
    fn query_opt(
        &mut self,
        sql: &str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Option<Row>, postgres::Error> {
        wait(
            self.runtime,
            self.deadline,
            self.client()?.query_opt(sql, params),
        )
    }
    fn batch_execute(&mut self, sql: &str) -> Result<(), postgres::Error> {
        wait(
            self.runtime,
            self.deadline,
            self.client()?.batch_execute(sql),
        )
    }
}
impl Drop for BoundedClient {
    fn drop(&mut self) {
        drop(self.client.take());
        if let Some(connection) = self.connection.take() {
            connection.abort();
            // Join actual cancellation. The connection future drops its socket
            // before this guard returns; timeout does not detach an SQL task.
            let _ = self.runtime.block_on(connection);
        }
    }
}

struct ResolverTask(JoinHandle<Result<Vec<IpAddr>, std::io::Error>>);
impl Drop for ResolverTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn resolve(host: &str) -> Result<Vec<IpAddr>, postgres::Error> {
    if let Ok(address) = host.parse::<IpAddr>() {
        return Ok(vec![address]);
    }
    static ADMISSION: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let admission = Arc::clone(ADMISSION.get_or_init(|| Arc::new(Semaphore::new(RESOLVERS))));
    let host = host.to_owned();
    resolve_blocking(admission, move || {
        (host.as_str(), 0)
            .to_socket_addrs()
            .map(|addresses| addresses.map(|address| address.ip()).collect())
    })
    .await
}

async fn resolve_blocking(
    admission: Arc<Semaphore>,
    resolve: impl FnOnce() -> Result<Vec<IpAddr>, std::io::Error> + Send + 'static,
) -> Result<Vec<IpAddr>, postgres::Error> {
    let permit = admission.try_acquire_owned().map_err(|_| unavailable())?;
    let mut task = ResolverTask(tokio::task::spawn_blocking(move || {
        // This permit stays held when a running native resolver outlives the
        // canceled caller. There are at most eight calls and no admitted queue.
        let _permit = permit;
        resolve()
    }));
    (&mut task.0)
        .await
        .map_err(|_| unavailable())?
        .map_err(|_| unavailable())
}

fn shuffle<T>(values: &mut [T]) -> Result<(), postgres::Error> {
    for index in (1..values.len()).rev() {
        let mut bytes = [0; std::mem::size_of::<usize>()];
        getrandom::fill(&mut bytes).map_err(|_| unavailable())?;
        let other = usize::from_ne_bytes(bytes) % (index + 1);
        values.swap(index, other);
    }
    Ok(())
}

async fn connect_candidates(
    source: Config,
    tls: postgres_native_tls::MakeTlsConnector,
) -> Result<
    (
        tokio_postgres::Client,
        tokio_postgres::Connection<
            postgres::Socket,
            postgres_native_tls::TlsStream<postgres::Socket>,
        >,
    ),
    postgres::Error,
> {
    let count = source.get_hosts().len().max(source.get_hostaddrs().len());
    if count == 0
        || (!source.get_hosts().is_empty()
            && !source.get_hostaddrs().is_empty()
            && source.get_hosts().len() != source.get_hostaddrs().len())
        || (source.get_ports().len() > 1 && source.get_ports().len() != count)
    {
        return Err(unavailable());
    }
    let mut indices: Vec<_> = (0..count).collect();
    if source.get_load_balance_hosts() == LoadBalanceHosts::Random {
        shuffle(&mut indices)?;
    }
    let mut last = None;
    for index in indices {
        let host = source.get_hosts().get(index);
        let port = candidate_port(&source, index);
        let mut addresses = match source.get_hostaddrs().get(index) {
            Some(address) => vec![Some(*address)],
            None => match host {
                Some(Host::Tcp(host)) => match resolve(host).await {
                    Ok(addresses) => addresses.into_iter().map(Some).collect(),
                    Err(error) => {
                        last = Some(error);
                        continue;
                    }
                },
                #[cfg(unix)]
                Some(Host::Unix(_)) => vec![None],
                _ => vec![],
            },
        };
        if source.get_load_balance_hosts() == LoadBalanceHosts::Random {
            shuffle(&mut addresses)?;
        }
        for address in addresses {
            let candidate = candidate_config(&source, host, address, port);
            match candidate.connect(tls.clone()).await {
                Ok(connection) => return Ok(connection),
                Err(error) => last = Some(error),
            }
        }
    }
    Err(last.unwrap_or_else(unavailable))
}

fn candidate_port(source: &Config, index: usize) -> u16 {
    source
        .get_ports()
        .get(index)
        .or_else(|| source.get_ports().first())
        .copied()
        .unwrap_or(5432)
}

// Copy every non-address setting in the pinned tokio-postgres Config. Preserve
// the original hostname for certificate verification/SNI; use only hostaddr
// for routing. The driver keeps TLS negotiation and target-session checks.
fn candidate_config(
    source: &Config,
    host: Option<&Host>,
    address: Option<IpAddr>,
    port: u16,
) -> Config {
    let mut config = Config::new();
    if let Some(user) = source.get_user() {
        config.user(user);
    }
    if let Some(password) = source.get_password() {
        config.password(password);
    }
    if let Some(dbname) = source.get_dbname() {
        config.dbname(dbname);
    }
    if let Some(options) = source.get_options() {
        config.options(options);
    }
    if let Some(name) = source.get_application_name() {
        config.application_name(name);
    }
    config
        .ssl_mode(source.get_ssl_mode())
        .ssl_negotiation(source.get_ssl_negotiation())
        .keepalives(source.get_keepalives())
        .keepalives_idle(source.get_keepalives_idle())
        .target_session_attrs(source.get_target_session_attrs())
        .channel_binding(source.get_channel_binding());
    if let Some(timeout) = source.get_connect_timeout() {
        config.connect_timeout(*timeout);
    }
    if let Some(timeout) = source.get_tcp_user_timeout() {
        config.tcp_user_timeout(*timeout);
    }
    if let Some(interval) = source.get_keepalives_interval() {
        config.keepalives_interval(interval);
    }
    if let Some(retries) = source.get_keepalives_retries() {
        config.keepalives_retries(retries);
    }
    match host {
        Some(Host::Tcp(host)) => {
            config.host(host);
        }
        #[cfg(unix)]
        Some(Host::Unix(path)) => {
            config.host_path(path);
        }
        None => {}
    }
    if let Some(address) = address {
        config.hostaddr(address);
    }
    config.port(port);
    config
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::too_many_lines)]
mod tests {
    use super::*;
    use std::{
        sync::{Condvar, Mutex, mpsc},
        time::Duration,
    };
    use tokio_postgres::config::{ChannelBinding, SslMode, SslNegotiation, TargetSessionAttrs};

    #[test]
    fn publication_expired_budget_never_polls_another_operation() {
        let expired = Instant::now() - Duration::from_secs(1);
        assert!(
            wait::<()>(runtime().unwrap(), expired, async {
                panic!("expired transport or SQL work must not begin")
            })
            .is_err()
        );
    }

    #[test]
    fn publication_candidates_preserve_transport_settings_and_host_ports() {
        let mut source = Config::new();
        source
            .user("reader")
            .password("test-only")
            .dbname("room")
            .options("-c search_path=public")
            .application_name("publication-test")
            .ssl_mode(SslMode::Require)
            .ssl_negotiation(SslNegotiation::Direct)
            .channel_binding(ChannelBinding::Require)
            .target_session_attrs(TargetSessionAttrs::ReadWrite)
            .keepalives(false)
            .keepalives_idle(Duration::from_secs(19))
            .keepalives_interval(Duration::from_secs(7))
            .keepalives_retries(4)
            .connect_timeout(Duration::from_secs(2))
            .tcp_user_timeout(Duration::from_secs(3))
            .host("first.example")
            .host("second.example")
            .port(15432)
            .port(25432)
            .load_balance_hosts(LoadBalanceHosts::Random);
        for index in 0..2 {
            let address: IpAddr = "127.0.0.1".parse().unwrap();
            let candidate = candidate_config(
                &source,
                source.get_hosts().get(index),
                Some(address),
                candidate_port(&source, index),
            );
            assert_eq!(candidate.get_hosts(), &source.get_hosts()[index..=index]);
            assert_eq!(candidate.get_hostaddrs(), &[address]);
            assert_eq!(candidate.get_ports(), &[source.get_ports()[index]]);
            assert_eq!(candidate.get_user(), source.get_user());
            assert_eq!(candidate.get_password(), source.get_password());
            assert_eq!(candidate.get_dbname(), source.get_dbname());
            assert_eq!(candidate.get_options(), source.get_options());
            assert_eq!(
                candidate.get_application_name(),
                source.get_application_name()
            );
            assert_eq!(candidate.get_ssl_mode(), source.get_ssl_mode());
            assert_eq!(
                candidate.get_ssl_negotiation(),
                source.get_ssl_negotiation()
            );
            assert_eq!(
                candidate.get_channel_binding(),
                source.get_channel_binding()
            );
            assert_eq!(
                candidate.get_target_session_attrs(),
                source.get_target_session_attrs()
            );
            assert_eq!(candidate.get_keepalives(), source.get_keepalives());
            assert_eq!(
                candidate.get_keepalives_idle(),
                source.get_keepalives_idle()
            );
            assert_eq!(
                candidate.get_keepalives_interval(),
                source.get_keepalives_interval()
            );
            assert_eq!(
                candidate.get_keepalives_retries(),
                source.get_keepalives_retries()
            );
            assert_eq!(
                candidate.get_connect_timeout(),
                source.get_connect_timeout()
            );
            assert_eq!(
                candidate.get_tcp_user_timeout(),
                source.get_tcp_user_timeout()
            );
            assert_eq!(
                candidate.get_load_balance_hosts(),
                LoadBalanceHosts::Disable
            );
        }
        let shared: Config = "host=one,two port=6543".parse().unwrap();
        assert_eq!(candidate_port(&shared, 1), 6543);
        let defaults: Config = "host=one,two".parse().unwrap();
        assert_eq!(candidate_port(&defaults, 1), 5432);
        #[cfg(unix)]
        {
            let unix: Config = "host=/tmp".parse().unwrap();
            let candidate = candidate_config(&unix, unix.get_hosts().first(), None, 5432);
            assert_eq!(candidate.get_hosts(), unix.get_hosts());
            assert!(candidate.get_hostaddrs().is_empty());
        }
    }

    #[test]
    fn publication_resolvers_retain_admission_until_native_work_ends() {
        let runtime = runtime().unwrap();
        let admission = Arc::new(Semaphore::new(RESOLVERS));
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        struct ReleaseOnDrop(Arc<(Mutex<bool>, Condvar)>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                let (lock, wake) = &*self.0;
                *lock.lock().unwrap() = true;
                wake.notify_all();
            }
        }
        let _cleanup = ReleaseOnDrop(Arc::clone(&release));
        let (entered, entry) = mpsc::channel();
        let (finished, finish) = mpsc::channel();
        for _ in 0..RESOLVERS {
            let entered = entered.clone();
            let finished = finished.clone();
            let release = Arc::clone(&release);
            let resolver = resolve_blocking(Arc::clone(&admission), move || {
                entered.send(()).unwrap();
                let (lock, wake) = &*release;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = wake.wait(released).unwrap();
                }
                finished.send(()).unwrap();
                Ok(vec![])
            });
            assert!(
                wait(
                    runtime,
                    Instant::now() + Duration::from_millis(25),
                    resolver
                )
                .is_err()
            );
            entry.recv_timeout(Duration::from_secs(1)).unwrap();
        }
        assert_eq!(admission.available_permits(), 0);
        assert!(
            wait(
                runtime,
                Instant::now() + Duration::from_millis(25),
                resolve_blocking(Arc::clone(&admission), || panic!(
                    "saturated DNS work must not queue"
                ))
            )
            .is_err()
        );
        assert_eq!(
            wait(
                runtime,
                Instant::now() + Duration::from_millis(25),
                resolve("127.0.0.1")
            )
            .unwrap(),
            vec!["127.0.0.1".parse::<IpAddr>().unwrap()]
        );
        let (lock, wake) = &*release;
        *lock.lock().unwrap() = true;
        wake.notify_all();
        for _ in 0..RESOLVERS {
            finish.recv_timeout(Duration::from_secs(1)).unwrap();
        }
        let until = Instant::now() + Duration::from_secs(1);
        while admission.available_permits() != RESOLVERS {
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    }
}
