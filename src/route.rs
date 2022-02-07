use std::collections::HashMap;

use lazy_static::lazy_static;
use regex::Regex;

use crate::api::{ApiDefinition, ApiMode};
use crate::endpoint::Endpoint;

lazy_static! {
    static ref IS_PARAM: Regex = Regex::new("\\{[^/]*\\}").unwrap();
}

#[derive(Debug)]
pub struct Node {
    endpoint_set: HashMap<String, Endpoint>,
    sub_route: HashMap<String, Self>,
    param: Option<Box<Self>>,
}

fn strip_path(path: &str) -> &str {
    if path.len() < 2 {
        return path;
    }
    if path.ends_with('/') {
        return &path[1..path.len() - 1];
    }
    &path[1..]
}

impl Node {
    fn empty() -> Self {
        Node {
            endpoint_set: HashMap::new(),
            sub_route: HashMap::new(),
            param: None,
        }
    }

    fn insert<'a>(&mut self, split_path: &mut impl Iterator<Item = &'a str>, endpoint: Endpoint) {
        match split_path.next() {
            None => {
                self.endpoint_set.insert(endpoint.method.clone(), endpoint);
            }
            Some(current_path) => {
                match IS_PARAM.is_match(current_path) {
                    false => match self.sub_route.get_mut(current_path) {
                        Some(next_node) => {
                            next_node.insert(split_path, endpoint);
                        }
                        None => {
                            let mut next_node = Node::empty();
                            next_node.insert(split_path, endpoint);
                            self.sub_route.insert(current_path.to_string(), next_node);
                        }
                    },
                    true => match &mut self.param {
                        Some(param) => {
                            param.insert(split_path, endpoint);
                        }
                        None => {
                            let mut next_node = Node::empty();
                            next_node.insert(split_path, endpoint);
                            self.param = Some(Box::new(next_node));
                        }
                    },
                };
            }
        };
    }

    pub fn new(api: &ApiDefinition) -> Self {
        let mut node = Node::empty();

        match &api.spec.mode {
            ApiMode::ForwardAll => (),
            ApiMode::ForwardStrict(endpoints) => {
                for endpoint in endpoints {
                    let mut built_endpoint = endpoint.clone();
                    built_endpoint.build_permission(&api.spec.app_name[1..]);
                    node.insert(
                        &mut strip_path(&built_endpoint.path.clone()).split('/'),
                        built_endpoint,
                    );
                }
            }
        }

        node
    }

    pub fn match_path(&self, path: &str, method: &str) -> Option<&Endpoint> {
        let mut split_path = strip_path(path).split('/');
        let mut node = self;
        loop {
            match split_path.next() {
                None => match node.endpoint_set.get(method) {
                    None => return None,
                    Some(endpoint) => return Some(endpoint),
                },
                Some(next_path) => match node.sub_route.get(next_path) {
                    Some(sub_node) => node = sub_node,
                    None => match &node.param {
                        None => return None,
                        Some(sub_node) => node = sub_node,
                    },
                },
            }
        }
    }
}
