//! TEI over gRPC, with a Rig embedding model shared by indexing and retrieval.

mod client;
mod embedding;
mod rpc;
mod types;

mod proto {
    tonic::include_proto!("tei.v1");
}

pub(crate) use client::TeiModel;
pub(crate) use types::ModelIdentity;
