//! Drag-and-drop source content. A drag must hand GTK its content when it
//! starts, but the item may not be downloaded yet: [`LazyFileProvider`]
//! announces the file-reference MIME types right away and writes them once
//! `prepare_copy` has materialised the file (drop targets read the data only
//! after the drop). Items that are already prepared get GDK's own
//! `GdkFileList` provider instead, which adds the portal file-transfer types
//! for sandboxed (Flatpak) targets.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use futures::future::{LocalBoxFuture, Shared};
use gtk::gdk::subclass::prelude::*;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use stickpile_core::PreparedCopy;

use crate::offers;

pub type PreparedFuture = Shared<LocalBoxFuture<'static, Result<PreparedCopy, String>>>;

mod imp {
    use std::cell::RefCell;

    use super::*;

    #[derive(Default)]
    pub struct LazyFileProvider {
        pub prepared: RefCell<Option<PreparedFuture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LazyFileProvider {
        const NAME: &'static str = "StickpileLazyFileProvider";
        type Type = super::LazyFileProvider;
        type ParentType = gdk::ContentProvider;
    }

    impl ObjectImpl for LazyFileProvider {}

    impl ContentProviderImpl for LazyFileProvider {
        fn formats(&self) -> gdk::ContentFormats {
            gdk::ContentFormats::new(&offers::DRAG_MIME_TYPES)
        }

        fn write_mime_type_future(
            &self,
            mime_type: &str,
            stream: &gio::OutputStream,
            io_priority: glib::Priority,
        ) -> Pin<Box<dyn Future<Output = Result<(), glib::Error>> + 'static>> {
            let prepared = self.prepared.borrow().clone();
            let mime = mime_type.to_string();
            let stream = stream.clone();
            Box::pin(async move {
                let prepared = prepared.ok_or_else(|| glib::Error::new(gio::IOErrorEnum::Failed, "nothing to drag"))?;
                let prepared = prepared
                    .await
                    .map_err(|e| glib::Error::new(gio::IOErrorEnum::Failed, &e))?;
                let bytes = offers::file_ref_payload(&mime, &prepared.file).ok_or_else(|| {
                    glib::Error::new(gio::IOErrorEnum::NotSupported, &format!("cannot provide {mime}"))
                })?;
                stream.write_all_future(bytes, io_priority).await.map_err(|(_, e)| e)?;
                Ok(())
            })
        }
    }
}

glib::wrapper! {
    pub struct LazyFileProvider(ObjectSubclass<imp::LazyFileProvider>) @extends gdk::ContentProvider;
}

impl LazyFileProvider {
    pub fn new(prepared: PreparedFuture) -> LazyFileProvider {
        let obj: LazyFileProvider = glib::Object::new();
        obj.imp().prepared.replace(Some(prepared));
        obj
    }
}

/// Content for a file that already exists on disk.
pub fn file_provider(path: &Path) -> gdk::ContentProvider {
    let list = gdk::FileList::from_array(&[gio::File::for_path(path)]);
    let mut providers = vec![gdk::ContentProvider::for_value(&list.to_value())];
    if let Some(gnome) = offers::gnome_copied_files(path) {
        providers.push(gdk::ContentProvider::for_bytes(
            offers::GNOME_COPIED_FILES,
            &glib::Bytes::from_owned(gnome),
        ));
    }
    gdk::ContentProvider::new_union(&providers)
}

/// Clipboard content from prepared offers: one `for_bytes` provider per
/// MIME type, in offer order.
pub fn clipboard_provider(specs: &[offers::OfferSpec]) -> gdk::ContentProvider {
    let providers: Vec<gdk::ContentProvider> = specs
        .iter()
        .map(|s| gdk::ContentProvider::for_bytes(&s.mime, &glib::Bytes::from(&s.data[..])))
        .collect();
    gdk::ContentProvider::new_union(&providers)
}
