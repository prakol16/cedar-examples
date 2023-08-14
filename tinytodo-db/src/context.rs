/*
 * Copyright 2022-2023 Amazon.com, Inc. or its affiliates. All Rights Reserved.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use cedar_db_example::expr_to_query::translate_response;
use itertools::Itertools;
use lazy_static::lazy_static;
use sea_query::{Alias, Query, SqliteQueryBuilder, SelectStatement};
use std::path::PathBuf;
use tracing::{info, trace};

use cedar_policy::{
    Authorizer, Context, Decision, Diagnostics, ParseErrors, PolicySet, Request,
    Schema, SchemaError, ValidationMode, Validator, CachedEntities,
};
use thiserror::Error;
use tokio::sync::{
    mpsc::{Receiver, Sender},
    oneshot,
};

use crate::{
    api::{
        AddShare, CreateList, CreateTask, DeleteList, DeleteShare, DeleteTask, Empty, GetList,
        GetLists, UpdateList, UpdateTask,
    },
    entitystore::{EntityDecodeError, EntityStore},
    objects::List,
    policy_store,
    util::{EntityUid, Lists, TYPE_USER, TYPE_TEAM},
};

// There's almost certainly a nicer way to do this than having separate `sender` fields

#[derive(Debug)]
pub enum AppResponse {
    GetList(Box<List>),
    Euid(EntityUid),
    Lists(Lists),
    TaskId(i64),
    Unit(()),
}

impl AppResponse {
    pub fn euid(v: impl Into<EntityUid>) -> Self {
        Self::Euid(v.into())
    }
}

impl TryInto<i64> for AppResponse {
    type Error = Error;

    fn try_into(self) -> std::result::Result<i64, Self::Error> {
        match self {
            AppResponse::TaskId(id) => Ok(id),
            _ => Err(Error::Type),
        }
    }
}

impl TryInto<List> for AppResponse {
    type Error = Error;

    fn try_into(self) -> std::result::Result<List, Self::Error> {
        match self {
            AppResponse::GetList(l) => Ok(*l),
            _ => Err(Error::Type),
        }
    }
}

impl TryInto<EntityUid> for AppResponse {
    type Error = Error;
    fn try_into(self) -> std::result::Result<EntityUid, Self::Error> {
        match self {
            AppResponse::Euid(e) => Ok(e),
            _ => Err(Error::Type),
        }
    }
}

impl TryInto<Empty> for AppResponse {
    type Error = Error;

    fn try_into(self) -> std::result::Result<Empty, Self::Error> {
        match self {
            AppResponse::Unit(()) => Ok(Empty::default()),
            _ => Err(Error::Type),
        }
    }
}

impl TryInto<Lists> for AppResponse {
    type Error = Error;
    fn try_into(self) -> std::result::Result<Lists, Self::Error> {
        match self {
            AppResponse::Lists(l) => Ok(l),
            _ => Err(Error::Type),
        }
    }
}

#[derive(Debug)]
pub enum AppQueryKind {
    // List CRUD
    CreateList(CreateList),
    GetList(GetList),
    UpdateList(UpdateList),
    DeleteList(DeleteList),

    // Task CRUD
    CreateTask(CreateTask),
    UpdateTask(UpdateTask),
    DeleteTask(DeleteTask),

    // Lists
    GetLists(GetLists),

    // Shares
    AddShare(AddShare),
    DeleteShare(DeleteShare),

    // Policy Set Updates
    UpdatePolicySet(PolicySet),
}

#[derive(Debug)]
pub struct AppQuery {
    kind: AppQueryKind,
    sender: oneshot::Sender<Result<AppResponse>>,
}

impl AppQuery {
    pub fn new(kind: AppQueryKind, sender: oneshot::Sender<Result<AppResponse>>) -> Self {
        Self { kind, sender }
    }
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("No Such Entity: {0}")]
    NoSuchEntity(EntityUid),
    #[error("Entity Decode Error: {0}")]
    EntityDecode(#[from] EntityDecodeError),
    #[error("Authorization Denied")]
    AuthDenied(Diagnostics),
    #[error("The list {0} does not contain a task with id {1}")]
    InvalidTaskId(EntityUid, i64),
    #[error("Internal Error")]
    TokioSend(#[from] tokio::sync::mpsc::error::SendError<AppQuery>),
    #[error("Internal Error")]
    TokioRecv(#[from] tokio::sync::oneshot::error::RecvError),
    #[error("Internal Error")]
    Type,
    #[error("Internal Error")]
    IO(#[from] std::io::Error),
    #[error("Error Parsing PolicySet: {0}")]
    Policy(#[from] ParseErrors),
    #[error("SQL error")]
    SQLError(#[from] rusqlite::Error),
}

impl Error {
    pub fn no_such_entity(euid: impl Into<EntityUid>) -> Self {
        Self::NoSuchEntity(euid.into())
    }
}

lazy_static! {
    pub static ref APPLICATION_TINY_TODO: EntityUid = r#"Application::"TinyTodo""#.parse().unwrap();
    static ref ACTION_EDIT_SHARE: EntityUid = r#"Action::"EditShare""#.parse().unwrap();
    static ref ACTION_UPDATE_TASK: EntityUid = r#"Action::"UpdateTask""#.parse().unwrap();
    static ref ACTION_CREATE_TASK: EntityUid = r#"Action::"CreateTask""#.parse().unwrap();
    static ref ACTION_DELETE_TASK: EntityUid = r#"Action::"DeleteTask""#.parse().unwrap();
    static ref ACTION_GET_LISTS: EntityUid = r#"Action::"GetLists""#.parse().unwrap();
    static ref ACTION_GET_LIST: EntityUid = r#"Action::"GetList""#.parse().unwrap();
    static ref ACTION_CREATE_LIST: EntityUid = r#"Action::"CreateList""#.parse().unwrap();
    static ref ACTION_UPDATE_LIST: EntityUid = r#"Action::"UpdateList""#.parse().unwrap();
    static ref ACTION_DELETE_LIST: EntityUid = r#"Action::"DeleteList""#.parse().unwrap();
}

pub struct AppContext {
    entities: EntityStore,
    authorizer: Authorizer,
    policies: PolicySet,
    schema: Schema,
    recv: Receiver<AppQuery>,
}

impl std::fmt::Debug for AppContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<AppContext>")
    }
}

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("{0}")]
    IO(#[from] std::io::Error),
    #[error("Error Parsing Schema: {0}")]
    Schema(#[from] SchemaError),
    #[error("Error Parsing PolicySet: {0}")]
    Policy(#[from] ParseErrors),
    #[error("Validation Failed: {0}")]
    Validation(String),
    #[error("Error Deserializing Json: {0}")]
    Json(#[from] serde_json::Error),
}

impl AppContext {
    // #[tracing::instrument(skip_all)]
    pub fn spawn(
        entities_path: impl Into<PathBuf>,
        schema_path: impl Into<PathBuf>,
        policies_path: impl Into<PathBuf>,
    ) -> std::result::Result<Sender<AppQuery>, ContextError> {
        info!("Starting server");

        let schema_path = schema_path.into();
        let policies_path = policies_path.into();
        let schema_file = std::fs::File::open(&schema_path)?;
        let schema = Schema::from_file(schema_file)?;

        // let entities_file = std::fs::File::open(entities_path.into())?;
        let entities = EntityStore::from_file(entities_path.into());

        let policy_src = std::fs::read_to_string(&policies_path)?;
        let policies = policy_src.parse()?;
        let validator = Validator::new(schema.clone());
        let output = validator.validate(&policies, ValidationMode::default());
        if output.validation_passed() {
            info!("Validation passed!");
            let authorizer = Authorizer::new();
            let (send, recv) = tokio::sync::mpsc::channel(100);
            let tx = send.clone();
            tokio::spawn(async move {
                info!("Serving application server!");
                policy_store::spawn_watcher(policies_path, schema_path, tx).await;
                let c = Self {
                    entities,
                    authorizer,
                    policies,
                    schema,
                    recv,
                };
                c.serve().await
            });

            Ok(send)
        } else {
            let error_string = output
                .validation_errors()
                .map(|err| format!("{err}"))
                .join("\n");
            Err(ContextError::Validation(error_string))
        }
    }

    #[tracing::instrument]
    async fn serve(mut self) -> Result<()> {
        loop {
            if let Some(msg) = self.recv.recv().await {
                let r = match msg.kind {
                    AppQueryKind::GetList(r) => self.get_list(r),
                    AppQueryKind::CreateList(r) => self.create_list(r),
                    AppQueryKind::UpdateList(r) => self.update_list(r),
                    AppQueryKind::DeleteList(r) => self.delete_list(r),
                    AppQueryKind::CreateTask(r) => self.create_task(r),
                    AppQueryKind::UpdateTask(r) => self.update_task(r),
                    AppQueryKind::DeleteTask(r) => self.delete_task(r),
                    AppQueryKind::GetLists(r) => self.get_lists(r),
                    AppQueryKind::AddShare(r) => self.add_share(r),
                    AppQueryKind::DeleteShare(r) => self.delete_share(r),
                    AppQueryKind::UpdatePolicySet(set) => self.update_policy_set(set),
                };
                if let Err(e) = msg.sender.send(r) {
                    trace!("Failed send response: {:?}", e);
                }
            }
        }
    }

    #[tracing::instrument(skip(policy_set))]
    fn update_policy_set(&mut self, policy_set: PolicySet) -> Result<AppResponse> {
        self.policies = policy_set;
        info!("Reloaded policy set");
        Ok(AppResponse::Unit(()))
    }

    fn add_share(&mut self, r: AddShare) -> Result<AppResponse> {
        self.is_authorized(&r.uid, &*ACTION_EDIT_SHARE, &r.list)?;
        // let list = self.entities.get_list(&r.list)?;
        // let team_uid = list.get_team(r.role).clone();
        // let target_entity = self.entities.get_user_or_team_mut(&r.share_with)?;
        // target_entity.insert_parent(team_uid);
        Ok(AppResponse::Unit(()))
    }

    fn delete_share(&mut self, r: DeleteShare) -> Result<AppResponse> {
        self.is_authorized(&r.uid, &*ACTION_EDIT_SHARE, &r.list)?;
        // let list = self.entities.get_list(&r.list)?;
        // let team_uid = list.get_team(r.role).clone();
        // let target_entity = self.entities.get_user_or_team_mut(&r.unshare_with)?;
        // target_entity.delete_parent(&team_uid);
        Ok(AppResponse::Unit(()))

    }

    fn update_task(&mut self, r: UpdateTask) -> Result<AppResponse> {
        self.is_authorized(&r.uid, &*ACTION_UPDATE_TASK, &r.list)?;
        if let Some(new_state) = r.state {
            self.entities.update_task(&r.list, r.task, new_state)?;
        }
        // TODO: allow update name
        Ok(AppResponse::Unit(()))
    }

    fn create_task(&mut self, r: CreateTask) -> Result<AppResponse> {
        self.is_authorized(&r.uid, &*ACTION_CREATE_TASK, &r.list)?;

        let task_id = self.entities.create_task(&r.list, r.name)?;
        Ok(AppResponse::TaskId(task_id))
    }

    fn delete_task(&mut self, r: DeleteTask) -> Result<AppResponse> {
        self.is_authorized(&r.uid, &*ACTION_DELETE_TASK, &r.list)?;
        self.entities.delete_task(&r.list, r.task)?;
        Ok(AppResponse::Unit(()))
    }

    fn get_lists(&self, r: GetLists) -> Result<AppResponse> {
        self.is_authorized(&r.uid, &*ACTION_GET_LISTS, &*APPLICATION_TINY_TODO)?;

        let mut query_expr = self.get_all_authorized_lists(&r.uid, &*ACTION_GET_LIST)?;
        let select = query_expr
            .column((Alias::new("resource"), Alias::new("uid")))
            .from_as(Alias::new("lists"), Alias::new("resource"))
            .to_string(SqliteQueryBuilder);

        info!("Running select query {}", select);
        let result = self.entities.get_lists(select)?;

        Ok(AppResponse::Lists(result.into()))
    }

    fn create_list(&mut self, r: CreateList) -> Result<AppResponse> {
        self.is_authorized(&r.uid, &*ACTION_CREATE_LIST, &*APPLICATION_TINY_TODO)?;
        let readers = self.entities.create_team()?;
        let editors = self.entities.create_team()?;

        let result = self.entities.create_list(r.uid, &r.name, readers, editors)?;
        Ok(AppResponse::euid(result))
    }

    fn get_list(&self, r: GetList) -> Result<AppResponse> {
        self.is_authorized(&r.uid, &*ACTION_GET_LIST, &r.list)?;
        let list = self.entities.get_list(&r.list)?;
        Ok(AppResponse::GetList(Box::new(list)))
    }

    fn update_list(&mut self, r: UpdateList) -> Result<AppResponse> {
        self.is_authorized(&r.uid, &*ACTION_UPDATE_LIST, &r.list)?;
        self.entities.update_list(&r.list, &r.name)?;
        Ok(AppResponse::Unit(()))
    }

    fn delete_list(&mut self, r: DeleteList) -> Result<AppResponse> {
        self.is_authorized(&r.uid, &*ACTION_DELETE_LIST, &r.list)?;
        self.entities.delete_list(&r.list)?;
        Ok(AppResponse::Unit(()))
    }

    pub fn get_all_authorized_lists(&self, principal: impl AsRef<EntityUid>, action: impl AsRef<EntityUid>) -> Result<SelectStatement> {
        let q = Request::builder()
            .principal(Some(principal.as_ref().clone().into()))
            .action(Some(action.as_ref().clone().into()))
            .resource_type("List".parse().unwrap())
            .build();
        let es = CachedEntities::cache_request(&self.entities, &q);
        let response = self.authorizer.is_authorized_parsed(&q, &self.policies, &es);
        match response {
            cedar_policy::PartialResponse::Concrete(response) => {
                Ok(Query::select().and_where((response.decision() == Decision::Allow).into()).to_owned())
            },
            cedar_policy::PartialResponse::Residual(res) => {
                Ok(translate_response(&res, &self.schema,
                    &|t1, t2| {
                    if *t1 == *TYPE_USER && *t2 == *TYPE_TEAM {
                        Ok((Alias::new("team_memberships"), Alias::new("user_uid"), Alias::new("team_uid")))
                    } else {
                        panic!("No tables available for membership test of types {:?} and {:?}", t1, t2)
                    }
                }).expect("Failed to translate residual policies"))
            },
        }
    }

    #[tracing::instrument(skip_all)]
    pub fn is_authorized(
        &self,
        principal: impl AsRef<EntityUid>,
        action: impl AsRef<EntityUid>,
        resource: impl AsRef<EntityUid>,
    ) -> Result<()> {
        let q = Request::new(
            Some(principal.as_ref().clone().into()),
            Some(action.as_ref().clone().into()),
            Some(resource.as_ref().clone().into()),
            Context::empty(),
        );
        let es = CachedEntities::cache_request(&self.entities, &q);
        info!(
            "is_authorized request: principal: {}, action: {}, resource: {}",
            principal.as_ref(),
            action.as_ref(),
            resource.as_ref()
        );
        let response = self.authorizer.is_authorized_full_parsed(&q, &self.policies, &es);
        info!("Auth response: {:?}", response);
        match response.decision() {
            Decision::Allow => Ok(()),
            Decision::Deny => Err(Error::AuthDenied(response.diagnostics().clone())),
        }
    }
}

// #[test]
// fn test_is_authorized_partial() {
//     let schema_path = "./tinytodo.cedarschema.json";
//     let entities_path = "./huge_entities.db";
//     let policies_path = "./policies.cedar";

//     let schema_path: PathBuf = schema_path.into();
//     let policies_path: PathBuf = policies_path.into();
//     let schema_file = std::fs::File::open(&schema_path).unwrap();
//     let schema = Schema::from_file(schema_file).unwrap();

//     let entities = EntityStore::from_file(<PathBuf as From<&str>>::from(entities_path));

//     let policy_src = std::fs::read_to_string(&policies_path).unwrap();
//     let policies = policy_src.parse().unwrap();
//     let validator = Validator::new(schema.clone());
//     let output = validator.validate(&policies, ValidationMode::default());
//     assert!(output.validation_passed());
//     let authorizer = Authorizer::new();

//     let principal: UserUid = <UserUid as TryFrom<EntityUid>>::try_from("User::\"a598f25b-727e-4d88-9df3-facba457ccea\"".parse().unwrap()).unwrap();
//     let action = &*ACTION_GET_LIST;

//     let query_expr: ConditionExpression = {
//         let q = Request::builder()
//             .principal(Some(principal.as_ref().clone().into()))
//             .action(Some(action.as_ref().clone().into()))
//             .resource_type("List".parse().unwrap())
//             .build();
//         let es = CachedEntities::cache_request(&entities, &q);
//         let response = authorizer.is_authorized_parsed(&q, &policies, &es);
//         match response {
//             cedar_policy::PartialResponse::Concrete(response) => {
//                 SimpleExpr::from(response.decision() == Decision::Allow).into()
//             },
//             cedar_policy::PartialResponse::Residual(res) => {
//                 let translated = translate_residual_policies(res, &schema,
//                     &|t1, t2| {
//                     if *t1 == *TYPE_USER && *t2 == *TYPE_TEAM {
//                         Ok((Alias::new("team_memberships"), Alias::new("user_uid"), Alias::new("team_uid")))
//                     } else {
//                         panic!("No tables available for membership test of types {:?} and {:?}", t1, t2)
//                     }
//                 });
//                 let mut cond = Condition::any();
//                 for c in translated.into_values() {
//                     cond = cond.add(c);
//                 }
//                 cond.into()
//             },
//         }
//     };

    // println!("Result: {}", Query::select()
    //     .column(Asterisk)
    //     .from_as(Alias::new("lists"), Alias::new("resource"))
    //     .cond_where(Condition::any().add(query_expr))
    //     .to_string(SqliteQueryBuilder))
// }
