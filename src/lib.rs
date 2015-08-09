// TODO: Use the json stream parser and write some macros!

extern crate url;
extern crate hyper;
extern crate rustc_serialize;
extern crate chrono;

#[macro_use]
extern crate log;
extern crate env_logger;

use rustc_serialize::json::{self, Json};

use url::Url;
use hyper::client::{Client, RedirectPolicy};
use chrono::naive::datetime::NaiveDateTime;
use std::iter;
use std::io;
use std::i64;
use std::io::Read;

pub use hyper::status::StatusCode as HttpStatus;
pub use rustc_serialize::json::ErrorCode as ParseError;

pub struct Aur {
    client: Client,
    base: Url,
}

// TODO HTTP2 Error?
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Ssl(Box<std::error::Error + Send + Sync>),
    Utf8(std::str::Utf8Error),
    Http {
        code: HttpStatus,
        message: String,
    },
    Aur(String),
    InvalidResponse,
    Parse {
        code: ParseError,
        line: usize, 
        col: usize,
    },
}

impl From<hyper::Error> for Error {
    fn from(e: hyper::Error) -> Self {
        use hyper::Error::*;
        match e {
            Utf8(e) => Error::Utf8(e),
            Io(e) => Error::Io(e),
            Ssl(e) => Error::Ssl(e),
            _ => panic!("BUG (in raur): unexpected error from hyper"),
        }
    }
}

impl From<json::ParserError> for Error {
    fn from(e: json::ParserError) -> Self {
        use rustc_serialize::json::ParserError::*;
        match e {
            SyntaxError(e, l, c) => Error::Parse { code: e, line: l, col: c },
            IoError(e) => Error::Io(e),
        }
    }
}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

#[derive(Clone, Debug)]
pub struct Package {
    pub base_name: String,
    pub base_id: u64,
    pub name: String,
    pub version: String,
    pub homepage: String,
    pub description: String,
    pub out_of_date: bool,

    pub created: NaiveDateTime,
    pub modified: NaiveDateTime,

    pub license: Option<String>,
    pub maintainer: Option<String>,
    pub votes: u64,
    pub id: u64,
    pub category_id: u64,
    pub download: String,
}

impl Package {
    fn from_json(j: Json) -> Result<Self, Error> {
        use rustc_serialize::json::Json::*;
        match j {
            // TODO: Checked casts in timestamps.
            Json::Object(mut h) => Ok(Package {
                base_name: match h.remove("PackageBase") {
                    Some(String(v)) => v,
                    _ => return Err(Error::InvalidResponse),
                },
                base_id: match h.remove("PackageBaseID") {
                    Some(U64(v)) => v,
                    _ => return Err(Error::InvalidResponse),
                },
                name: match h.remove("Name") {
                    Some(String(v)) => v,
                    _ => return Err(Error::InvalidResponse),
                },
                category_id: match h.remove("CategoryID") {
                    Some(U64(v)) => v,
                    _ => return Err(Error::InvalidResponse),
                },
                description: match h.remove("Description") {
                    Some(String(v)) => v,
                    _ => return Err(Error::InvalidResponse),
                },
                created: match h.remove("FirstSubmitted") {
                    Some(U64(v)) if v <= (i64::MAX as u64) => NaiveDateTime::from_timestamp(v as i64, 0),
                    _ => return Err(Error::InvalidResponse),
                },
                modified: match h.remove("LastModified") {
                    Some(U64(v)) if v <= (i64::MAX as u64) => NaiveDateTime::from_timestamp(v as i64, 0),
                    _ => return Err(Error::InvalidResponse),
                },
                id: match h.remove("ID") {
                    Some(U64(v)) => v,
                    _ => return Err(Error::InvalidResponse),
                },
                license: match h.remove("License") {
                    Some(String(v)) => Some(v),
                    Some(Null) => None,
                    _ => return Err(Error::InvalidResponse),
                },
                maintainer: match h.remove("Maintainer") {
                    Some(String(v)) => Some(v),
                    Some(Null) => None,
                    _ => return Err(Error::InvalidResponse),
                },
                votes: match h.remove("NumVotes") {
                    Some(U64(v)) => v,
                    _ => return Err(Error::InvalidResponse),
                },
                out_of_date: match h.remove("OutOfDate") {
                    Some(U64(v)) => v != 0,
                    _ => return Err(Error::InvalidResponse),
                },
                homepage: match h.remove("URL") {
                    Some(String(v)) => v,
                    _ => return Err(Error::InvalidResponse),
                },
                download: match h.remove("URLPath") {
                    Some(String(v)) => v,
                    _ => return Err(Error::InvalidResponse),
                },
                version: match h.remove("Version") {
                    Some(String(v)) => v,
                    _ => return Err(Error::InvalidResponse),
                },
            }),
            _ => {
                debug!("Expected object, got: {:?}", j);
                Err(Error::InvalidResponse)
            }
        }
    }
}

impl Aur {
    /// Create a new AUR client.
    pub fn new() -> Aur {
        let mut aur = Aur {
            client: Client::new(),
            base: Url::parse("https://aur4.archlinux.org/rpc.php").unwrap(),
        };
        aur.client.set_redirect_policy(RedirectPolicy::FollowAll);
        aur
    }

    /// Search the AUR.
    pub fn search(&self, pat: &str) -> Result<Vec<Package>, Error> {
        match try!(self.call_one("search", pat)) {
            Json::Array(a) => Ok(try!(a.into_iter().map(Package::from_json).collect())),
            _ => Err(Error::InvalidResponse),
        }
    }

    /// Search the AUR by maintainer.
    pub fn msearch(&self, author: &str) -> Result<Vec<Package>, Error> {
        match try!(self.call_one("msearch", author)) {
            Json::Array(a) => Ok(try!(a.into_iter().map(Package::from_json).collect())),
            _ => Err(Error::InvalidResponse),
        }
    }

    /// Retrieve information for the named package.
    pub fn info(&self, name: &str) -> Result<Option<Package>, Error> {
        let pkg = try!(self.call_one("info", name));
        if pkg.as_array().map(|v|v.is_empty()).unwrap_or(false) {
            Ok(None)
        } else {
            Package::from_json(pkg).map(|v|Some(v))
        }
    }

    /// Retrieve information for the named packages.
    pub fn multiinfo<'a, I>(&self, names: I) -> Result<Vec<Package>, Error>
        where I: IntoIterator<Item = &'a str>,
    {
        match try!(self.call_multi("multiinfo", names)) {
            Json::Array(a) => Ok(try!(a.into_iter().map(Package::from_json).collect())),
            _ => Err(Error::InvalidResponse),
        }
    }

    fn call_one(&self, fun: &str, arg: &str) -> Result<Json, Error> {
        let mut url = self.base.clone();
        url.set_query_from_pairs([("type", fun), ("arg", arg)].into_iter().cloned());
        self.rpc(url)
    }
    
    fn call_multi<'a, I>(&self, fun: &'a str, args: I) -> Result<Json, Error>
        where I: IntoIterator<Item = &'a str>,
    {
        let mut url = self.base.clone();
        let iter = iter::once(("type", fun)).chain(iter::repeat("arg[]").zip(args.into_iter()));
        url.set_query_from_pairs(iter);
        self.rpc(url)
    }

    fn rpc(&self, url: Url) -> Result<Json, Error> {
        let mut response = try!(self.client.get(url).send());
        if !response.status.is_success() {
            let mut msg = if let Some(&hyper::header::ContentLength(len)) = response.headers.get() {
                // TODO: Safe cast?
                String::with_capacity(len as usize)
            } else {
                String::new()
            };
            try!(response.read_to_string(&mut msg));
            return Err(Error::Http {
                code: response.status,
                message: msg
            })
        }

        let mut obj = match try!(Json::from_reader(&mut response)) {
            Json::Object(h) => h,
            other => {
                debug!("Got invalid response from server: {:?}", other);
                return Err(Error::InvalidResponse);
            }
        };

        let (typ, result) = if let (Some(t), Some(r)) = (obj.remove("type"), obj.remove("results")) {
            (t, r)
        } else {
            debug!("Got invalid response from server: {:?}", obj);
            return Err(Error::InvalidResponse);
        };

        return match typ.as_string() {
            Some("error") => Err(Error::Aur(match result {
                Json::String(s) => s,
                r => r.to_string(),
            })),
            None => {
                debug!("Bad type from server: {:?}", typ);
                Err(Error::InvalidResponse)
            },
            _ => {
                trace!("{:#?}", result);
                Ok(result)
            }
        }
    }
}
