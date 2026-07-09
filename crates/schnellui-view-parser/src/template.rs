//! Compiler-independent template data used by the dev socket client.
use crate::{AttrName, AttrValue, Node, ViewTree};
use proc_macro2::TokenStream;
use quote::{quote, ToTokens};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TemplateBlueprint {
    pub id: String,
    pub nodes: Vec<BlueprintNode>,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BlueprintNode {
    pub path: Vec<u32>,
    pub tag: String,
    pub text: Option<String>,
    pub attrs: Vec<BlueprintAttr>,
    pub dynamic: String,
    pub inert: bool,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BlueprintAttr {
    pub name: String,
    pub value: String,
    pub dynamic: bool,
}
impl ViewTree {
    /// Produces data for a mounted macro site. `id` is its compiled file:line:column.
    /// Paths address source children, including content consumed by leaf constructors.
    pub fn blueprint(&self, id: impl Into<String>) -> TemplateBlueprint {
        fn visit(node: &Node, path: Vec<u32>, out: &mut Vec<BlueprintNode>) {
            let mut record = BlueprintNode {
                path: path.clone(),
                tag: String::new(),
                text: None,
                attrs: Vec::new(),
                dynamic: String::new(),
                inert: node.is_static(),
            };
            match node {
                Node::StaticText(text) => {
                    record.tag = "text".into();
                    record.text = Some(text.value());
                }
                Node::Dynamic(expr) => {
                    record.tag = "text".into();
                    record.dynamic = expr.to_token_stream().to_string();
                }
                Node::Element(el) => {
                    record.tag = el.tag.to_string();
                    for attr in &el.attrs {
                        let name = match &attr.name {
                            AttrName::Prop(name) => name.to_string(),
                            AttrName::Event(name) => format!("on:{name}"),
                        };
                        let (value, dynamic) = match &attr.value {
                            AttrValue::Static(syn::Lit::Str(s)) => (s.value(), false),
                            AttrValue::Static(lit) => (lit.to_token_stream().to_string(), false),
                            AttrValue::Flag => ("true".into(), false),
                            AttrValue::Dynamic(expr) => (expr.to_token_stream().to_string(), true),
                        };
                        record.attrs.push(BlueprintAttr {
                            name,
                            value,
                            dynamic,
                        });
                    }
                    // Content leaves consume their child; it is the same retained node.
                    if !is_container(&record.tag) {
                        if let [child] = el.children.as_slice() {
                            match child {
                                Node::StaticText(text) => record.text = Some(text.value()),
                                Node::Dynamic(expr) => {
                                    record.dynamic = expr.to_token_stream().to_string()
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            let descend = is_container(&record.tag);
            out.push(record);
            if descend {
                if let Node::Element(el) = node {
                    for (index, child) in el.children.iter().enumerate() {
                        let mut child_path = path.clone();
                        child_path.push(index as u32);
                        visit(child, child_path, out);
                    }
                }
            }
        }
        let mut nodes = Vec::new();
        for (index, root) in self.roots.iter().enumerate() {
            visit(root, vec![index as u32], &mut nodes);
        }
        TemplateBlueprint {
            id: id.into(),
            nodes,
        }
    }
}
pub(crate) fn is_container(tag: &str) -> bool {
    matches!(
        tag,
        "column"
            | "row"
            | "stack"
            | "scroll"
            | "flex"
            | "pad"
            | "component_ref"
            | "referenced"
            | "panel"
            | "dialog"
            | "context_menu"
            | "tabs"
            | "tab_bar"
            | "tabbar"
            | "grouped_tabs"
            | "grouped_tab_list"
            | "grouped_tablist"
            | "tab_group"
            | "tabgroup"
            | "tab_node"
            | "tabnode"
            | "tree_tab"
            | "list"
            | "table"
            | "table_row"
            | "tr"
            | "dock_area"
    )
}
pub(crate) fn emit_template(tree: &ViewTree) -> TokenStream {
    let blueprint = tree.blueprint("");
    let nodes = blueprint.nodes.iter().map(|node| {
        let path = &node.path; let tag = &node.tag; let dynamic = &node.dynamic; let inert = node.inert;
        let text = match &node.text { Some(text) => quote!(Some(#text)), None => quote!(None) };
        let attrs = node.attrs.iter().map(|attr| { let name = &attr.name; let value = &attr.value; let dynamic = attr.dynamic; quote! { ::schnellui::widgets::TemplateAttr { name: #name, value: #value, dynamic: #dynamic } } });
        quote! { ::schnellui::widgets::TemplateNode { path: &[#(#path),*], tag: #tag, text: #text, attrs: &[#(#attrs),*], dynamic: #dynamic, inert: #inert } }
    });
    quote! { ::schnellui::widgets::Template { id: concat!(file!(), ":", line!(), ":", column!()), nodes: &[#(#nodes),*] } }
}

// Leaf constructors consume their label rather than emitting a child view. Keep
// the typed constructor while reading its literal from the same hoisted skeleton.
pub(crate) fn replace_static_literal(
    tokens: TokenStream,
    text: &syn::LitStr,
    replacement: TokenStream,
) -> TokenStream {
    let needle = text.to_token_stream().to_string();
    let mut out = TokenStream::new();
    for token in tokens {
        match token {
            proc_macro2::TokenTree::Literal(lit) if lit.to_string() == needle => {
                out.extend(replacement.clone())
            }
            proc_macro2::TokenTree::Group(group) => {
                let mut updated = proc_macro2::Group::new(
                    group.delimiter(),
                    replace_static_literal(group.stream(), text, replacement.clone()),
                );
                updated.set_span(group.span());
                out.extend([proc_macro2::TokenTree::Group(updated)]);
            }
            other => out.extend([other]),
        }
    }
    out
}
