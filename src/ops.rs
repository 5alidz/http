use std::path::PathBuf;
use iron::modifiers::Header;
use self::super::{Options, Error};
use mime_guess::guess_mime_type_opt;
use self::super::util::{url_path, html_response, file_contains, percent_decode, ERROR_HTML, DIRECTORY_LISTING_HTML};
use iron::{headers, status, method, mime, IronResult, Listening, Response, TypeMap, Request, Handler, Iron};


#[derive(Clone, Hash, Ord, PartialOrd, Eq, PartialEq)]
pub struct HttpHandler {
    pub hosted_directory: (String, PathBuf),
    pub follow_symlinks: bool,
}

impl HttpHandler {
    pub fn new(opts: &Options) -> HttpHandler {
        HttpHandler {
            hosted_directory: opts.hosted_directory.clone(),
            follow_symlinks: opts.follow_symlinks,
        }
    }
}

impl Handler for HttpHandler {
    fn handle(&self, req: &mut Request) -> IronResult<Response> {
        match req.method {
            method::Options => self.handle_options(req),
            method::Get => self.handle_get(req),
            method::Head => {
                self.handle_get(req).map(|mut r| {
                    r.body = None;
                    r
                })
            }
            method::Trace => self.handle_trace(req),
            _ => self.handle_bad_method(req),
        }
    }
}

impl HttpHandler {
    fn handle_options(&self, req: &mut Request) -> IronResult<Response> {
        println!("{} asked for options", req.remote_addr);
        Ok(Response::with((status::Ok, Header(headers::Allow(vec![method::Options, method::Get, method::Head, method::Trace])))))
    }

    fn handle_get(&self, req: &mut Request) -> IronResult<Response> {
        let (req_p, symlink, url_err) =
            req.url.path().into_iter().filter(|p| !p.is_empty()).fold((self.hosted_directory.1.clone(), false, false), |(mut cur, mut sk, mut err), pp| {
                if let Some(pp) = percent_decode(pp) {
                    cur.push(&*pp);
                } else {
                    err = true;
                }

                if let Ok(meta) = cur.metadata() {
                    sk = sk || meta.file_type().is_symlink();
                }

                (cur, sk, err)
            });

        if url_err {
            self.handle_invalid_url(req, "<p>Percent-encoding decoded to invalid UTF-8.</p>")
        } else if !req_p.exists() || (symlink && !self.follow_symlinks) {
            self.handle_get_nonexistant(req, req_p)
        } else if req_p.is_file() {
            self.handle_get_file(req, req_p)
        } else {
            self.handle_get_dir(req, req_p)
        }
    }

    fn handle_invalid_url(&self, req: &mut Request, cause: &str) -> IronResult<Response> {
        println!("{} requested with invalid URL {}", req.remote_addr, req.url);
        Ok(Response::with((status::BadRequest,
                           "text/html;charset=utf-8".parse::<mime::Mime>().unwrap(),
                           html_response(ERROR_HTML, &["400 Bad Request", "The request URL couldn't be parsed.", cause]))))
    }

    fn handle_get_nonexistant(&self, req: &mut Request, req_p: PathBuf) -> IronResult<Response> {
        println!("{} requested nonexistant file {}", req.remote_addr, req_p.display());
        Ok(Response::with((status::NotFound,
                           "text/html;charset=utf-8".parse::<mime::Mime>().unwrap(),
                           html_response(ERROR_HTML,
                                         &["404 Not Found", &format!("The requested entity \"{}\" doesn't exist.", url_path(&req.url)), ""]))))
    }

    fn handle_get_file(&self, req: &mut Request, req_p: PathBuf) -> IronResult<Response> {
        let mime_type = guess_mime_type_opt(&req_p).unwrap_or_else(|| if file_contains(&req_p, 0) {
            "application/octet-stream".parse().unwrap()
        } else {
            "text/plain".parse().unwrap()
        });
        println!("{} was served file {} as {}", req.remote_addr, req_p.display(), mime_type);
        Ok(Response::with((status::Ok, mime_type, req_p)))
    }

    fn handle_get_dir(&self, req: &mut Request, req_p: PathBuf) -> IronResult<Response> {
        let relpath = (url_path(&req.url) + "/").replace("//", "/");
        println!("{} was served directory listing for {}", req.remote_addr, req_p.display());
        Ok(Response::with((status::Ok,
                           "text/html;charset=utf-8".parse::<mime::Mime>().unwrap(),
                           html_response(DIRECTORY_LISTING_HTML,
                                         &[&relpath,
                                           &req_p.read_dir()
                                               .unwrap()
                                               .map(Result::unwrap)
                                               .filter(|f| self.follow_symlinks || !f.metadata().unwrap().file_type().is_symlink())
                                               .fold("".to_string(), |cur, f| {
                let fname = f.file_name().into_string().unwrap() +
                            if !f.file_type().unwrap().is_file() {
                    "/"
                } else {
                    ""
                };
                cur + "<li><a href=\"" + &format!("/{}", relpath).replace("//", "/") + &fname + "\">" + &fname + "</a></li>\n"
            })]))))
    }

    fn handle_trace(&self, req: &mut Request) -> IronResult<Response> {
        println!("{} requested TRACE", req.remote_addr);

        let mut hdr = req.headers.clone();
        hdr.set(headers::ContentType("message/http".parse().unwrap()));

        Ok(Response {
            status: Some(status::Ok),
            headers: hdr,
            extensions: TypeMap::new(),
            body: None,
        })
    }

    fn handle_bad_method(&self, req: &mut Request) -> IronResult<Response> {
        println!("{} used invalid request method {}", req.remote_addr, req.method);
        Ok(Response::with((status::NotImplemented,
                           "text/html;charset=utf-8".parse::<mime::Mime>().unwrap(),
                           html_response(ERROR_HTML,
                                         &["501 Not Implemented",
                                           "This operation was not implemented.",
                                           &format!("<p>Unsupported request method: {}.<br />Supported methods: OPTIONS, GET, HEAD and TRACE.</p>",
                                                    req.method)]))))
    }
}


pub fn try_ports<H: Handler + Clone>(hndlr: H, from: u16, up_to: u16) -> Result<Listening, Error> {
    for port in from..up_to {
        match Iron::new(hndlr.clone()).http(("0.0.0.0", port)) {
            Ok(server) => return Ok(server),
            Err(error) => {
                if !error.to_string().contains("port") {
                    return Err(Error::Io {
                        desc: "server",
                        op: "start",
                        more: None,
                    });
                }
            }
        }
    }

    Err(Error::Io {
        desc: "server",
        op: "start",
        more: Some("no free ports"),
    })
}
