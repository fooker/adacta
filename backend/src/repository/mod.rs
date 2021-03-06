use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{anyhow, Result};
use futures::TryStreamExt;
use log::info;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};

use crate::config::Repository as Config;
use crate::meta::Metadata;
use crate::proto::model::{DocId, Kind};

trait Filename {
    fn filename(&self) -> OsString;
}

pub trait BundleState {
    fn path(repository: &Repository) -> PathBuf;
}

pub struct Staging {}

impl BundleState for Staging {
    fn path(repository: &Repository) -> PathBuf {
        return repository.path.as_ref().as_ref().join("staging");
    }
}

pub struct Inboxed {}

impl BundleState for Inboxed {
    fn path(repository: &Repository) -> PathBuf {
        return repository.path.as_ref().as_ref().join("inbox");
    }
}

pub struct Archived {}

impl BundleState for Archived {
    fn path(repository: &Repository) -> PathBuf {
        return repository.path.as_ref().as_ref().join("archive");
    }
}

pub struct Bundle<'r, State: BundleState> {
    id: DocId,
    repository: &'r Repository,
    state: PhantomData<State>,
}

pub struct Repository {
    path: Box<dyn AsRef<Path> + Send + Sync>,
}

pub struct Inbox<'r>(&'r Repository);

impl<'r> Inbox<'r> {
    pub async fn list(&self) -> Result<Vec<Bundle<'r, Inboxed>>> {
        let list = tokio::fs::read_dir(Inboxed::path(self.0)).await?
            .err_into::<anyhow::Error>()
            .and_then(|entry| async move {
                let time = entry.metadata().await?.modified()?;

                let id = DocId::from_str(entry.file_name().to_string_lossy().as_ref())?;
                let bundle = Bundle {
                    id,
                    repository: &self.0,
                    state: PhantomData::default(),
                };

                return Ok(((time, bundle.id), bundle));
            })
            .try_collect::<BTreeMap<_, _>>().await?;

        return Ok(list.into_iter().map(|(_, id)| id).collect());
    }

    pub async fn get(&self, id: DocId) -> Option<Bundle<'r, Inboxed>> {
        let bundle = Bundle {
            id,
            repository: &self.0,
            state: PhantomData::default(),
        };

        let metadata = tokio::fs::metadata(&bundle.path()).await;
        if metadata.is_err() {
            return None;
        }

        return Some(bundle);
    }
}

pub struct Archive<'r>(&'r Repository);

impl<'r> Archive<'r> {
    pub async fn get(&self, id: DocId) -> Option<Bundle<'r, Archived>> {
        let bundle = Bundle {
            id,
            repository: &self.0,
            state: PhantomData::default(),
        };

        let metadata = tokio::fs::metadata(&bundle.path()).await;
        if metadata.is_err() {
            return None;
        }

        return Some(bundle);
    }
}

impl Filename for Kind {
    fn filename(&self) -> OsString {
        return match self {
            Self::Document => OsString::from("document.pdf"),
            Self::Preview => OsString::from("preview.png"),
            Self::Plaintext => OsString::from("document.txt"),
            Self::Metadata => OsString::from("metadata.json"),
            Self::Other { name } => OsString::from(name),
        };
    }
}

impl Filename for DocId {
    fn filename(&self) -> OsString { return self.to_string().into(); }
}

impl<State: BundleState> Bundle<'_, State> {
    pub fn id(&self) -> &DocId { return &self.id; }

    pub fn path(&self) -> PathBuf { return State::path(self.repository).join(self.id.filename()); }

    pub fn path_of(&self, kind: impl Borrow<Kind>) -> PathBuf { return self.path().join(kind.borrow().filename()); }

    pub async fn read(&self, kind: impl Borrow<Kind>) -> Result<Option<impl AsyncRead>> {
        let path = self.path_of(kind);

        info!("Reading fragment {:?}", path);
        let file = OpenOptions::new()
            .read(true)
            .open(path)
            .await;

        match file {
            Ok(file) => {
                return Ok(Some(file));
            }

            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }

            Err(err) => {
                return Err(err.into());
            }
        }
    }

    pub async fn read_plaintext(&self) -> Result<String> {
        let mut file = self.read(Kind::Plaintext).await?
            .ok_or_else(|| anyhow!("Plaintext missing in bundle: {}", self.id))?;

        let mut buffer = String::new();
        file.read_to_string(&mut buffer).await?;

        return Ok(buffer);
    }

    pub async fn read_metadata(&self) -> Result<Metadata> {
        let file = self.read(Kind::Metadata).await?
            .ok_or_else(|| anyhow!("Metadata missing in bundle: {}", self.id))?;

        return Metadata::load(file).await;
    }
}

impl Repository {
    pub async fn from_config(config: Config) -> Result<Self> {
        return Self::with_path(config.path).await;
    }

    pub async fn with_path(path: impl AsRef<Path> + Send + Sync + 'static) -> Result<Self> {
        info!("Opening repository at {:?}", path.as_ref());

        // Create repository path if missing
        tokio::fs::create_dir_all(&path).await?;

        return Ok(Self { path: Box::new(path) });
    }

    pub fn path(&self) -> &Path { return self.path.as_ref().as_ref(); }

    pub fn inbox(&self) -> Inbox<'_> {
        return Inbox(self);
    }

    pub fn archive(&self) -> Archive<'_> {
        return Archive(self);
    }

    pub async fn stage(&self) -> Result<Bundle<'_, Staging>> {
        let bundle = Bundle {
            id: DocId::random(),
            repository: self,
            state: Default::default(),
        };

        info!("Creating staged bundle {:?}", bundle.path());
        tokio::fs::create_dir_all(&bundle.path()).await?;

        return Ok(bundle);
    }
}

impl<'r> Bundle<'r, Inboxed> {
    pub async fn archive(self) -> Result<Bundle<'r, Archived>> {
        let archived = Bundle {
            id: self.id,
            repository: self.repository,
            state: PhantomData::default(),
        };

        info!("Archiving inboxed bundle {:?} -> {:?}", self.path(), archived.path());

        tokio::fs::create_dir_all(archived.path().parent().expect("No parent directory")).await?;
        tokio::fs::rename(&self.path(), &archived.path()).await?;

        return Ok(archived);
    }

    pub async fn delete(self) -> Result<()> {
        info!("Deleting inboxed bundle {:?}", self.path());
        tokio::fs::remove_dir_all(&self.path()).await?;

        return Ok(());
    }
}

impl<'r> Bundle<'r, Staging> {
    pub async fn create(self) -> Result<Bundle<'r, Inboxed>> {
        let inboxed = Bundle {
            id: self.id,
            repository: self.repository,
            state: PhantomData::default(),
        };

        info!("Inboxing staged bundle {:?} -> {:?}", self.path(), inboxed.path());
        tokio::fs::create_dir_all(inboxed.path().parent().expect("No parent directory")).await?;
        tokio::fs::rename(&self.path(), &inboxed.path()).await?;

        return Ok(inboxed);
    }

    pub async fn write(&self, kind: Kind) -> Result<impl AsyncWrite> {
        let path = self.path().join(kind.filename());

        info!("Writing fragment {:?} to {:?}", kind, path);
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .await?;

        return Ok(file);
    }

    pub async fn delete(self) -> Result<()> {
        info!("Deleting staged bundle {:?}", self.path());
        tokio::fs::remove_dir_all(&self.path()).await?;

        return Ok(());
    }
}

impl<'r> Bundle<'r, Inboxed> {
    pub async fn write_metadata(&self, metadata: &Metadata) -> Result<()> {
        let path = self.path().join(Kind::Metadata.filename());

        info!("Writing metadata fragment to {:?}", path);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .await?;

        metadata.save(file).await?;

        return Ok(());
    }
}
