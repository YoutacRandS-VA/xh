use anyhow::{anyhow, Result};
use reqwest::blocking::{Request, Response};
use reqwest::header::{
    HeaderMap, AUTHORIZATION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, LOCATION,
    PROXY_AUTHORIZATION, TRANSFER_ENCODING, WWW_AUTHENTICATE,
};
use reqwest::{Method, StatusCode, Url};

use crate::middleware::{Middleware, Next};
use crate::utils::clone_request;

pub struct RedirectFollower<T>
where
    T: FnMut(Response, &mut Request) -> Result<()>,
{
    max_redirects: usize,
    callback: Option<T>,
}

impl<T> RedirectFollower<T>
where
    T: FnMut(Response, &mut Request) -> Result<()>,
{
    pub fn new(max_redirects: usize) -> Self {
        RedirectFollower {
            max_redirects,
            callback: None,
        }
    }

    pub fn on_redirect(&mut self, callback: T) {
        self.callback = Some(callback);
    }
}

impl<T> Middleware for RedirectFollower<T>
where
    T: FnMut(Response, &mut Request) -> Result<()>,
{
    fn handle(&mut self, mut first_request: Request, mut next: Next) -> Result<Response> {
        // This buffers the body in case we need it again later
        // reqwest does *not* do this, it ignores 307/308 with a streaming body
        let mut request = clone_request(&mut first_request)?;
        let mut response = next.run(first_request)?;
        let mut remaining_redirects = self.max_redirects - 1;

        while let Some(mut next_request) = get_next_request(request, &response) {
            if remaining_redirects > 0 {
                remaining_redirects -= 1;
            } else {
                return Err(anyhow!(
                    "Too many redirects (--max-redirects={})",
                    self.max_redirects
                ));
            }
            if let Some(ref mut callback) = self.callback {
                callback(response, &mut next_request)?;
            }
            request = clone_request(&mut next_request)?;
            response = next.run(next_request)?;
        }

        Ok(response)
    }
}

// See https://github.com/seanmonstar/reqwest/blob/bbeb1ede4e8098481c3de6f2cafb8ecca1db4ede/src/async_impl/client.rs#L1500-L1607
fn get_next_request(mut request: Request, response: &Response) -> Option<Request> {
    let get_next_url = |request: &Request| {
        response
            .headers()
            .get(LOCATION)
            .and_then(|location| location.to_str().ok())
            .and_then(|location| request.url().join(location).ok())
    };

    match response.status() {
        StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND | StatusCode::SEE_OTHER => {
            let next_url = get_next_url(&request)?;
            let prev_url = request.url();
            if is_cross_domain_redirect(&next_url, prev_url) {
                remove_sensitive_headers(request.headers_mut());
            }
            remove_content_headers(request.headers_mut());
            *request.url_mut() = next_url;
            *request.body_mut() = None;
            *request.method_mut() = match *request.method() {
                Method::GET => Method::GET,
                Method::HEAD => Method::HEAD,
                _ => Method::GET,
            };
            Some(request)
        }
        StatusCode::TEMPORARY_REDIRECT | StatusCode::PERMANENT_REDIRECT => {
            let next_url = get_next_url(&request)?;
            let prev_url = request.url();
            if is_cross_domain_redirect(&next_url, prev_url) {
                remove_sensitive_headers(request.headers_mut());
            }
            *request.url_mut() = next_url;
            Some(request)
        }
        _ => None,
    }
}

// See https://github.com/seanmonstar/reqwest/blob/bbeb1ede4e8098481c3de6f2cafb8ecca1db4ede/src/redirect.rs#L234-L246
fn is_cross_domain_redirect(next: &Url, previous: &Url) -> bool {
    next.host_str() != previous.host_str()
        || next.port_or_known_default() != previous.port_or_known_default()
}

// See https://github.com/seanmonstar/reqwest/blob/bbeb1ede4e8098481c3de6f2cafb8ecca1db4ede/src/redirect.rs#L234-L246
fn remove_sensitive_headers(headers: &mut HeaderMap) {
    headers.remove(AUTHORIZATION);
    headers.remove(COOKIE);
    headers.remove("cookie2");
    headers.remove(PROXY_AUTHORIZATION);
    headers.remove(WWW_AUTHENTICATE);
}

// See https://github.com/seanmonstar/reqwest/blob/bbeb1ede4e8098481c3de6f2cafb8ecca1db4ede/src/async_impl/client.rs#L1503-L1510
fn remove_content_headers(headers: &mut HeaderMap) {
    headers.remove(TRANSFER_ENCODING);
    headers.remove(CONTENT_ENCODING);
    headers.remove(CONTENT_TYPE);
    headers.remove(CONTENT_LENGTH);
}
