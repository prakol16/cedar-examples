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

use std::{collections::{HashMap, HashSet}, borrow::Cow, path::Path};
use lazy_static::lazy_static;
use cedar_db_example::sqlite::{EntitySQLInfo, AncestorSQLInfo, EntitySQLId};
use rusqlite::{Connection, params, OptionalExtension};
use thiserror::Error;
use uuid::Uuid;

use cedar_policy::{EvaluationError, EntityDatabase, ParsedEntity, EntityId};
use serde::{Deserialize, Serialize};

use crate::{
    context::{Error, APPLICATION_TINY_TODO},
    objects::{List, Application, Task, TaskState},
    util::{EntityUid, ListUid, TeamUid, UserUid, TYPE_USER, TYPE_TEAM, TYPE_LIST, TYPE_APP},
};

pub struct EntityStore {
    conn: Connection
}

lazy_static! {
    static ref USERS_TABLE_INFO: EntitySQLInfo<'static> = EntitySQLInfo::simple("users", vec!["name"], None);
    static ref USERS_TEAM_MEMBERSHIPS: AncestorSQLInfo<'static> = AncestorSQLInfo::new("team_memberships", "user_uid", "team_uid");

    static ref TEAM_TABLE_INFO: EntitySQLInfo<'static> = EntitySQLInfo::simple("teams", vec![], None);
    static ref TEAM_MEMBERSHIPS: AncestorSQLInfo<'static> = AncestorSQLInfo::new("subteams", "child_team", "parent_team");

    static ref LIST_TABLE_INFO: EntitySQLInfo<'static> = EntitySQLInfo::new("lists", "uid",
        vec!["text", "name", "owner", "readers", "editors"],
        vec![(0, "text"), (1, "name")],
        None);
}

impl EntityDatabase for EntityStore {

    fn get<'e>(&'e self, uid: &cedar_policy::EntityUid) -> Result<Option<Cow<'e, ParsedEntity>>, EvaluationError> {
        // println!("Executing fetch for {:?}", uid);
        match uid.type_name() {
            t if *t == *TYPE_USER => {
                let mut ancestors = USERS_TEAM_MEMBERSHIPS.get_ancestors(&self.conn, uid.id(), &TYPE_TEAM).map_err(EvaluationError::mk_err)?;
                ancestors.extend([uid.clone(), APPLICATION_TINY_TODO.clone().into()]);
                Ok(USERS_TABLE_INFO.make_entity(&self.conn, uid, |_| Ok(ancestors)).map_err(EvaluationError::mk_err)?.map(Cow::Owned))
            },
            t if *t == *TYPE_TEAM => {
                let mut ancestors = TEAM_MEMBERSHIPS.get_ancestors(&self.conn, uid.id(), &TYPE_TEAM).map_err(EvaluationError::mk_err)?;
                ancestors.insert(APPLICATION_TINY_TODO.clone().into());
                Ok(TEAM_TABLE_INFO.make_entity(&self.conn, uid, |_| Ok(ancestors)).map_err(EvaluationError::mk_err)?.map(Cow::Owned))
            },
            t if *t == *TYPE_LIST => {
                Ok(self.get_list(&EntityUid(uid.clone()).try_into().unwrap()).ok().map(|l| Cow::Owned(l.into())))
            },
            t if *t == *TYPE_APP => Ok(Some(Cow::Owned(Application::default().into()))),
            t if t.basename() == "Action" => Ok(Some(Cow::Owned(ParsedEntity::new(uid.clone(), HashMap::new(), HashSet::new())))),
            _ => Ok(None)
        }
    }

    fn partial_mode(&self) -> cedar_policy::Mode {
        cedar_policy::Mode::Concrete
    }
}

impl EntityStore {
    pub fn from_file(file: impl AsRef<Path>) -> Self {
        Self::new(Connection::open(file).expect("Failed to open database"))
    }

    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    pub fn create_team(&mut self) -> Result<TeamUid, Error> {
        let fresh_uid = Uuid::new_v4().to_string();
        self.conn.execute("INSERT INTO teams VALUES (?)", &[&fresh_uid])?;
        Ok(fresh_uid.parse::<EntityId>().unwrap().into())
    }

    pub fn create_list(&mut self, owner: UserUid, name: &str, readers: TeamUid, editors: TeamUid) -> Result<ListUid, Error> {
        let fresh_uid = Uuid::new_v4().to_string();
        self.conn.execute("INSERT INTO lists VALUES (?, ?, ?, ?, ?)",
        &[
            &fresh_uid,
            owner.as_ref().id().as_ref(),
            name,
            readers.as_ref().id().as_ref(),
            editors.as_ref().id().as_ref()
        ]).unwrap();
        Ok(fresh_uid.parse::<EntityId>().unwrap().into())
    }

    fn get_tasks(&self, euid: &ListUid) -> Result<Vec<Task>, Error> {
        let mut stmt = self.conn.prepare("SELECT ROWID, name, state FROM tasks WHERE list_uid = ?")?;
        let result = stmt.query_map(&[euid.as_ref().id().as_ref()], |row| {
            Ok(Task::new(
                row.get(0)?,
                row.get(1)?,
                row.get::<_, bool>(2)?.into()
            ))
        })?
        .collect::<Result<Vec<Task>, _>>()?;
        Ok(result)
    }

    pub fn get_list(&self, euid: &ListUid) -> Result<List, Error> {
        let tasks = self.get_tasks(euid)?;
        self.conn.query_row("SELECT owner, name, readers, editors FROM lists WHERE uid = ?", [euid.as_ref().id().as_ref()],
        |row| {
            let owner: EntitySQLId = row.get(0)?;
            let readers: EntitySQLId = row.get(2)?;
            let editors: EntitySQLId = row.get(3)?;
            Ok(List::new(
                euid.clone(),
                owner.id().into(),
                row.get(1)?,
                tasks,
                readers.id().into(),
                editors.id().into(),
            ))
        })
        .optional()
        .unwrap()
        .ok_or(Error::no_such_entity(euid.clone()))
    }

    pub fn get_lists(&self, query: String) -> Result<Vec<EntityUid>, Error> {
        let mut query_prepared = self.conn.prepare(&query)?;
        let r: Result<Vec<EntityUid>, rusqlite::Error> = query_prepared.query_map([], |row| {
            let uid: EntitySQLId = row.get(0)?;
            Ok(ListUid::from(uid.id()).into())
        })?
        .collect();
        Ok(r?)
    }

    pub fn update_list(&self, list: &ListUid, name: &str) -> Result<(), Error> {
        self.conn.execute("UPDATE lists SET name = ? WHERE uid = ?", &[name, list.as_ref().id().as_ref()])?;
        Ok(())
    }

    pub fn delete_list(&self, list: &ListUid) -> Result<(), Error> {
        self.conn.execute("DELETE FROM lists WHERE uid = ?", &[list.as_ref().id().as_ref()])?;
        Ok(())
    }

    pub fn create_task(&self, list: &ListUid, name: String) -> Result<i64, Error> {
        self.conn.execute("INSERT INTO tasks VALUES (?, ?, ?)",
            params![name, false, list.as_ref().id().as_ref()])?;
        Ok(self.conn.query_row("SELECT last_insert_rowid()", [], |row| row.get::<_, i64>(0))?)
    }

    pub fn update_task(&self, list: &ListUid, uid: i64, new_state: TaskState) -> Result<(), Error> {
        self.conn.execute("UPDATE tasks SET state = ? WHERE ROWID = ? AND list_uid = ?",
            params![new_state == TaskState::Checked, uid, list.as_ref().id().as_ref()])?;
        Ok(())
    }

    pub fn delete_task(&self, list: &ListUid, uid: i64) -> Result<(), Error> {
        let num_changed = self.conn.execute("DELETE FROM tasks WHERE ROWID = ? AND list_uid = ?", params![uid, list.as_ref().id().as_ref()])?;
        if num_changed == 0 {
            Err(Error::InvalidTaskId(list.clone().into(), uid))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum EntityType {
    List,
    User,
    Team,
    Application,
}

#[derive(Debug, Clone, Error)]
pub enum EntityDecodeError {
    #[error("The following required attribute was missing: {0}")]
    MissingAttr(&'static str),
    #[error("Evaluation Failed: {0}")]
    Eval(#[from] EvaluationError),
    #[error("Field {0} was wrong typed. Expected {0}")]
    WrongType(&'static str, &'static str),
    #[error("Enum was not one of required fields. Enum{enumeration}, Got {got}")]
    BadEnum {
        enumeration: &'static str,
        got: String,
    },
}

#[cfg(test)]
mod test {
    use cedar_policy::{Authorizer, PolicySet, Response, Request, Context};

    use super::*;

    fn is_authorized(
        es: &EntityStore,
        policy_set: &PolicySet,
        authorizer: &Authorizer,
        principal: cedar_policy::EntityUid,
        action: cedar_policy::EntityUid,
        resource: cedar_policy::EntityUid,
    ) -> Response {
        let q = Request::new(
            Some(principal),
            Some(action),
            Some(resource),
            Context::empty(),
        );
        authorizer.is_authorized_full_parsed(&q, policy_set, es)
    }

    #[test]
    fn test_basic() {
        let store = EntityStore::from_file("entities.db");

        let authorizer = Authorizer::new();

        let policy_src = std::fs::read_to_string(&"policies.cedar").expect("policies.cedar file should exist");
        let policies: PolicySet = policy_src.parse().expect("policies should parse correctly");

        println!("Is authorized: {:?}",
            is_authorized(
                &store,
                &policies,
                &authorizer,
                "User::\"aaron\"".parse().unwrap(),
                "Action::\"GetList\"".parse().unwrap(),
                "List::\"l0\"".parse().unwrap()
            )
        );

        println!("Is authorized 2 {:?}",
            is_authorized(&store, &policies, &authorizer, "User::\"andrew\"".parse().unwrap(),
            "Action::\"GetList\"".parse().unwrap(),
            "List::\"bhDbo6AjP613Lccz\"".parse().unwrap()));
    }

}