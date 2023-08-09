import sqlite3
import uuid
from faker import Faker
import random
from pathlib import Path
import json

class Team:
    def __init__(self, name: str, parents) -> None:
        self.name = name
        self.parents: list[Team] = parents

    def __repr__(self) -> str:
        return f"Team({self.name})"

    def as_euid(self) -> str:
        return f'Team::"{self.name}"'

    def to_dict(self) -> dict:
        return {
            'uid': self.as_euid(),
            'parents': [team.as_euid() for team in self.parents] + ['Application::"TinyTodo"']
        }

team_temp = Team("temp", [])
team_admin = Team("admin", [])
team_interns = Team("interns", [team_temp])

default_teams = [team_temp, team_admin, team_interns]

def add_teams_to_table(teams: list[Team]) -> None:
    cur.executemany("""INSERT INTO teams VALUES (?)""", [(team.name,) for team in teams])
    con.commit()

def add_default_teams_to_table() -> None:
    add_teams_to_table(default_teams)
    subteams = [
        (team.name, parent.name) for team in default_teams for parent in team.parents
    ]
    cur.executemany("""INSERT INTO subteams VALUES (?, ?)""", subteams)
    con.commit()

def get_random_teams(extra_teams: list[Team]) -> list[Team]:
    return random.choice([
        [team_temp],
        [team_admin],
        [team_interns, team_temp],
    ]) + random.sample(extra_teams, random.randint(0, min(len(extra_teams), 3)))  # extra teams should not have any edges between them

def setup_tables() -> None:
    cur.execute("""
        CREATE TABLE users (
            uid TEXT PRIMARY KEY,
            name TEXT NOT NULL
        )
    """)
    cur.execute("""
        CREATE TABLE team_memberships (
            user_uid REFERENCES users,
            team_uid REFERENCES teams
        )
    """)
    cur.execute("CREATE TABLE lists (uid text PRIMARY KEY, owner REFERENCES users, name text NOT NULL, readers REFERENCES teams, editors REFERENCES teams)")
    cur.execute("""CREATE TABLE teams (uid text PRIMARY KEY)""")
    cur.execute("CREATE TABLE subteams (child_team REFERENCES teams, parent_team REFERENCES teams)")
    cur.execute("CREATE TABLE tasks (name text NOT NULL, state bool NOT NULL, list_uid REFERENCES lists)")
    con.commit()

class User:
    def __init__(self, name: str) -> None:
        self.uid = uuid.uuid4()
        self.name = name
        self.teams = []

    def set_teams(self, teams: list[Team]) -> None:
        self.teams = teams

    def __repr__(self) -> str:
        return f"User({self.name}, {self.teams})"

    def to_tuple(self) -> tuple[str, str]:
        return (str(self.uid), self.name)

    def as_euid(self) -> str:
        return f'User::"{self.uid}"'

    def to_dict(self) -> dict:
        return {
            'euid': self.as_euid(),
            'name': self.name,
            'parents': [team.as_euid() for team in self.teams] + ['Application::"TinyTodo"']
        }

class Task:
    def __init__(self, name: str) -> None:
        self.name = name

    def to_tuple(self, lst_id: uuid.UUID) -> tuple[str, bool, str]:
        return (self.name, False, str(lst_id))

    def to_dict(self, i: int) -> dict:
        return {
            'name': self.name,
            'state': 'Unchecked',
            'id': i
        }

class FactorizationTaskList:
    def __init__(self, owner: User, readers: Team, editors: Team, start: int, end: int) -> None:
        self.uid = uuid.uuid4()
        self.owner = owner
        self.name = f'Factorize the numbers from {start:,} to {end:,}'
        self.readers = readers
        self.editors = editors
        self.start = start
        self.end = end

    def __repr__(self) -> str:
        return f"FactorizationTaskList({self.name}, {self.owner.name}, {self.readers.name}, {self.editors.name})"

    def to_tuple(self) -> tuple[str, str, str, str, str]:
        return (str(self.uid), str(self.owner.uid), self.name, self.readers.name, self.editors.name)

    def as_euid(self) -> str:
        return f'List::"{self.uid}"'

    def generate_tasks(self) -> list[Task]:
        return [
            Task(f'Factorize the number {i:,}') for i in range(self.start, self.end + 1)
        ]

    def to_dict(self) -> dict:
        return {
            'uid': self.as_euid(),
            'owner': self.owner.as_euid(),
            'name': self.name,
            'readers': self.readers.as_euid(),
            'editors': self.editors.as_euid(),
            'tasks': [task.to_dict(i) for i, task in enumerate(self.generate_tasks())],
        }

def create_random_team() -> Team:
    return Team(str(uuid.uuid4()), [])

def create_random_team_or_existing(p: float, collecting: list[Team]) -> Team:
    if random.random() > p:
        result = create_random_team()
        collecting.append(result)
        return result
    else:
        return random.choice(default_teams)

def create_random_list(users: list[User], collection: list[Team]) -> FactorizationTaskList:
    """Creates a random team"""
    owner = random.choice(users)
    readers = create_random_team_or_existing(0.0001, collection)
    editors = create_random_team_or_existing(0.0001, collection)
    start = random.randint(2**63, 2**64 - 1)
    end = start + random.randint(5, 10)
    return FactorizationTaskList(owner, readers, editors, start, end)


def create_random_lists(users: list[User], n: int) -> tuple[list[FactorizationTaskList], list[Team]]:
    collection = []
    result = [create_random_list(users, collection) for _ in range(n)]
    return result, collection


def create_random_user() -> User:
    """Create a random user"""
    return User(fake.name())

def create_random_users(n: int) -> list[User]:
    """Create n randomly generated users"""
    return [create_random_user() for _ in range(n)]

def add_users_to_table(users: list[User]) -> None:
    """Create a user table"""
    cur.executemany("""
        INSERT INTO users VALUES (?, ?)
    """, [user.to_tuple() for user in users])

    team_memberships = [
        (str(user.uid), team.name) for user in users for team in user.teams
    ]
    cur.executemany("""
        INSERT INTO team_memberships VALUES (?, ?)
    """, team_memberships)
    con.commit()

def add_lists_to_table(lists: list[FactorizationTaskList]) -> None:
    cur.executemany("""
        INSERT INTO lists VALUES (?, ?, ?, ?, ?)
    """, [list.to_tuple() for list in lists])
    con.commit()

def add_tasks_to_table(lists: list[FactorizationTaskList]) -> None:
    cur.executemany("""
        INSERT INTO tasks VALUES (?, ?, ?)
    """, [task.to_tuple(list.uid) for list in lists for task in list.generate_tasks()])
    con.commit()

def add_to_tables(users: list[User], lists: list[FactorizationTaskList], extra_teams: list[Team]) -> None:
    global cur, con

    entites_file = Path("./entities.huge.db")

    # If the file already exists, remove it
    entites_file.unlink(missing_ok=True)

    con = sqlite3.connect(entites_file)
    cur = con.cursor()

    setup_tables()
    add_users_to_table(users)
    add_default_teams_to_table()
    add_teams_to_table(extra_teams)
    add_lists_to_table(lists)
    add_tasks_to_table(lists)

def write_json(users: list[User], lists: list[FactorizationTaskList], extra_teams: list[Team]) -> None:
    with open('../tinytodo/entities.huge.json', 'w') as f:
        json.dump({
            'users': { user.as_euid(): user.to_dict() for user in users },
            'lists': { list.as_euid(): list.to_dict() for list in lists },
            'teams': { team.as_euid(): team.to_dict() for team in extra_teams },
            'app': { 'euid': 'Application::"TinyTodo"' }
        }, f, indent=4)


def main():
    global fake

    fake = Faker(use_weighting=False)

    fake.seed(0xcedaa)
    random.seed(0xcedaa)

    users = create_random_users(100000)
    lists, extra_teams = create_random_lists(users, 100000)
    for user in users:
        user.set_teams(get_random_teams(extra_teams))

    add_to_tables(users, lists, extra_teams)
    write_json(users, lists, extra_teams)

if __name__ == "__main__":
    main()
