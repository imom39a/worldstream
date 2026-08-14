#!/usr/bin/env python3
"""PROTOTYPE / THROWAWAY — accepted Counter Room commit seam probe (Python 3.12)."""
from __future__ import annotations
import argparse, contextlib, hashlib, json, os, shutil, socket, sqlite3, subprocess, tempfile, threading
from dataclasses import dataclass, asdict
from pathlib import Path
from typing import Any, Callable, Iterator
try: import psycopg2
except ImportError: psycopg2 = None

def canonical(x: Any) -> str:
    """Recursive canonical bytes; this is the common caller/stimulus/transition codec."""
    return json.dumps(x, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
def h(x: Any) -> str: return hashlib.sha256(canonical(x).encode()).hexdigest()


def core_digest(state: dict[str, Any]) -> str:
    return h({"domain": "worldstream/core-state/v1", "state": state})


def activity_digest(state: dict[str, Any]) -> str:
    return h(
        {
            "domain": "worldstream/activity-state/v1",
            "pack_digest": "CounterRoom/v1",
            "state": state,
        }
    )


def authoritative_digest(core_hash: str, activity_hash: str) -> str:
    return h(
        {
            "domain": "worldstream/authoritative-state/v1",
            "core": core_hash,
            "activity": activity_hash,
        }
    )
@dataclass(frozen=True)
class Head:
    seq: int; transition: str; core: str; activity: str; authoritative: str
    def token(self) -> dict[str, Any]: return asdict(self)
GENESIS_CORE = {"room_status": "active"}
GENESIS_ACTIVITY = {"counter": 0, "deadline": 100}
GENESIS_CORE_HASH = core_digest(GENESIS_CORE)
GENESIS_ACTIVITY_HASH = activity_digest(GENESIS_ACTIVITY)
GENESIS_AUTHORITATIVE_HASH = authoritative_digest(
    GENESIS_CORE_HASH, GENESIS_ACTIVITY_HASH
)
GENESIS = Head(
    0,
    h({"domain":"CounterRoom/Genesis/v1","room":"counter","core":GENESIS_CORE_HASH,"activity":GENESIS_ACTIVITY_HASH,"authoritative":GENESIS_AUTHORITATIVE_HASH}),
    GENESIS_CORE_HASH,
    GENESIS_ACTIVITY_HASH,
    GENESIS_AUTHORITATIVE_HASH,
)
@dataclass(frozen=True)
class Prepared:
    op: str; caller_hash: str; base: Head; healthy: int; integrity_gen: int; authority_gen: int; policy_rev: int; catching_up: int
    stimulus: str; bundle: str  # opaque canonical bytes; adapters do not interpret action kinds
class Clock:
    def __init__(self, now=0): self.now=now


class TimerConditionMiss(Exception):
    def __init__(self, outcome: str, reason: str):
        super().__init__(reason)
        self.outcome = outcome
        self.reason = reason

def head_from(r: dict[str,Any]) -> Head: return Head(r["seq"],r["transition_hash"],r["core_hash"],r["activity_hash"],r["authoritative_hash"])

def prepare(snapshot: dict[str,Any], op: str, request: dict[str,Any], admitted_at: int) -> Prepared | dict[str,Any]:
    """The entire shared Counter domain. It is pure and runs before a database lane exists."""
    if not snapshot.get("materialization_valid", True):
        return {"outcome": "Fault", "reason": "materialization-hash-mismatch"}
    caller_hash=h(request)  # deliberately excludes host time
    base=head_from(snapshot); witnesses={k:snapshot[k] for k in ("healthy","integrity_gen","authority_gen","policy_rev","catching_up")}
    a=dict(request); kind=a.get("kind"); stimulus={"request":a,"participant_admitted_at":admitted_at}
    # Timer semantic time belongs to the immutable schedule, not HostClock/native DB time.
    if kind=="fire": stimulus={"timer_fired":{"room":"counter","timer_id":a["timer_id"],"generation":a["generation"],"scheduled_for":a["scheduled_for"]},"semantic_at":a["scheduled_for"]}
    stimulus_bytes=canonical(stimulus)
    def p(bundle): return Prepared(op,caller_hash,base,**witnesses,stimulus=stimulus_bytes,bundle=canonical(bundle))
    if not snapshot["healthy"]: return {"outcome":"Fault","reason":"unhealthy-domain"}
    if kind in ("receipt","reject"):
        return p({"type":"disposition","result":{"kind":"NoChange" if kind=="receipt" else "Rejection"},"timers":[],"frame":None,"activation":None})
    if snapshot["archive"]:
        if kind == "advance":
            return p({"type":"disposition","result":{"kind":"Rejection","reason":"room-archived"},"timers":[],"frame":None,"activation":None})
        if kind == "fire":
            return {"outcome":"NotApplicable","reason":"stale-timer-witness"}
        return {"outcome":"Fenced","reason":"archived-core"}
    if snapshot["catching_up"]: return {"outcome":"Reprepare","reason":"CatchingUp"}
    if kind=="advance":
        if admitted_at>=snapshot["deadline"]:
            return p({"type":"disposition","result":{"kind":"Rejection","reason":"deadline"},"timers":[],"frame":None,"activation":None})
        delta=int(a["delta"]); timer_changes=[]
    elif kind=="schedule":
        generations=[generation for (timer_id,generation) in snapshot["timers"] if timer_id==a["timer_id"]]
        expected_generation=max(generations,default=0)+1
        if a["generation"]!=expected_generation: return {"outcome":"Fault","reason":"timer-generation-not-next"}
        if a["scheduled_for"]<=admitted_at: return {"outcome":"Fault","reason":"timer-not-strictly-forward"}
        delta=0; timer_changes=[{"op":"insert","timer_id":a["timer_id"],"generation":a["generation"],"scheduled_for":a["scheduled_for"],"state":"scheduled"}]
    elif kind=="cancel":
        t=snapshot["timers"].get((a["timer_id"],a["generation"]))
        if not t or t["state"]!="scheduled": return {"outcome":"Fault","reason":"invalid-pack-timer-cancel"}
        delta=0; timer_changes=[{"op":"state","timer_id":a["timer_id"],"generation":a["generation"],"expected_state":"scheduled","expected_scheduled_for":t["scheduled_for"],"state":"cancelled","on_miss":"Fault"}]
    elif kind=="reschedule":
        old_generation=a["generation"]; next_generation=a["next_generation"]
        t=snapshot["timers"].get((a["timer_id"],old_generation))
        generations=[generation for (timer_id,generation) in snapshot["timers"] if timer_id==a["timer_id"]]
        if not t or t["state"]!="scheduled": return {"outcome":"Fault","reason":"invalid-pack-timer-reschedule"}
        if next_generation!=max(generations,default=0)+1: return {"outcome":"Fault","reason":"timer-generation-not-next"}
        if a["scheduled_for"]<=admitted_at: return {"outcome":"Fault","reason":"timer-not-strictly-forward"}
        delta=0; timer_changes=[
            {"op":"state","timer_id":a["timer_id"],"generation":old_generation,"expected_state":"scheduled","expected_scheduled_for":t["scheduled_for"],"state":"cancelled","on_miss":"Fault"},
            {"op":"insert","timer_id":a["timer_id"],"generation":next_generation,"scheduled_for":a["scheduled_for"],"state":"scheduled"},
        ]
    elif kind=="fire":
        t=snapshot["timers"].get((a["timer_id"],a["generation"]))
        if admitted_at<a["scheduled_for"]: return {"outcome":"NotApplicable","reason":"timer-not-due"}
        if not t or t["scheduled_for"]!=a["scheduled_for"] or t["state"]!="scheduled": return {"outcome":"NotApplicable","reason":"stale-timer-witness"}
        delta=int(a.get("delta",1)); timer_changes=[{"op":"state","timer_id":a["timer_id"],"generation":a["generation"],"expected_state":"scheduled","expected_scheduled_for":a["scheduled_for"],"state":"fired","on_miss":"NotApplicable","miss_precedence":"before_head"}]
    elif kind=="archive":
        delta=0
        timer_changes=[
            {"op":"state","timer_id":timer_id,"generation":generation,"expected_state":"scheduled","expected_scheduled_for":timer["scheduled_for"],"state":"cancelled","on_miss":"Fault"}
            for (timer_id,generation), timer in sorted(snapshot["timers"].items())
            if timer["state"]=="scheduled"
        ]
    else: return {"outcome":"Fault","reason":"impossible-stimulus"}
    core={"room_status":"archived" if kind=="archive" else "active"}
    activity={"counter":snapshot["counter"]+delta,"deadline":snapshot["deadline"]}
    core_hash,activity_hash=core_digest(core),activity_digest(activity)
    authoritative={"core":core_hash,"activity":activity_hash}
    authoritative_hash=authoritative_digest(core_hash, activity_hash)
    attention="room-wide-archive-fence" if kind=="archive" else "counter-attention"
    transition_hash=h({"domain":"CounterRoom/Transition/v1","room":"counter","seq":base.seq+1,"prior_transition":base.transition,"stimulus":stimulus,"timer_changes":timer_changes,"attention":attention,"core":core_hash,"activity":activity_hash,"authoritative":authoritative_hash})
    nh=Head(base.seq+1,transition_hash,core_hash,activity_hash,authoritative_hash)
    transition={"seq":nh.seq,"parent":base.token(),"head":nh.token(),"stimulus":stimulus_bytes,"timer_changes":timer_changes,"attention":attention,"core":core,"activity":activity,"authoritative":authoritative}
    return p({"type":"advance","result":{"kind":"Advance","head":nh.token(),"counter":activity["counter"]},"transition":transition,"room":{"counter":activity["counter"],"deadline":activity["deadline"],"archive":int(core["room_status"]=="archived")},"timers":timer_changes,"frame":{"head":nh.token(),"core":core,"activity":activity},"activation":{"intent":"activate" if core["room_status"]=="active" else "archive","decision":"accepted"},"fence_room":kind=="archive"})

class DB:
    def __init__(
        self,
        c: Any,
        kind: str,
        *,
        target_version: int = 2,
        fail_v2: bool = False,
    ):
        self.c = c
        self.kind = kind
        self.ph = "%s" if kind == "postgres" else "?"
        self.migrate(target_version=target_version, fail_v2=fail_v2)
    def x(self,sql,args=()):
        cur=self.c.cursor();cur.execute(sql.replace("?",self.ph),args);return cur
    def migrate(self, *, target_version: int, fail_v2: bool):
        if self.kind=="sqlite": self.c.execute("PRAGMA foreign_keys=ON")
        self.x("CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY)")
        if not self.x("SELECT 1 FROM schema_migrations WHERE version=1").fetchone():
            self.x("CREATE TABLE rooms(room TEXT PRIMARY KEY,seq INTEGER NOT NULL,transition_hash TEXT NOT NULL,core_hash TEXT NOT NULL,activity_hash TEXT NOT NULL,authoritative_hash TEXT NOT NULL,counter INTEGER NOT NULL,deadline INTEGER NOT NULL,healthy INTEGER NOT NULL,integrity_gen INTEGER NOT NULL,authority_gen INTEGER NOT NULL,policy_rev INTEGER NOT NULL,catching_up INTEGER NOT NULL,archive INTEGER NOT NULL)")
            self.x("CREATE TABLE legacy_receipts(op TEXT PRIMARY KEY,caller_hash TEXT NOT NULL,codec TEXT NOT NULL)")
            self.x("CREATE TABLE legacy_timers(room TEXT,timer_id TEXT,generation INTEGER,scheduled_for INTEGER,state TEXT,PRIMARY KEY(room,timer_id,generation))")
            self.x("CREATE TABLE legacy_transitions(room TEXT,seq INTEGER,parent_head TEXT NOT NULL,head TEXT NOT NULL,stimulus TEXT NOT NULL,timer_changes TEXT NOT NULL,attention TEXT NOT NULL,core TEXT NOT NULL,activity TEXT NOT NULL,authoritative TEXT NOT NULL,PRIMARY KEY(room,seq))")
            self.x("INSERT INTO schema_migrations VALUES(1)")
            self.c.commit()
        if target_version == 1:
            return
        if not self.x("SELECT 1 FROM schema_migrations WHERE version=2").fetchone():
            try:
                if self.kind=="sqlite": self.c.execute("BEGIN")
                self.x("CREATE TABLE transitions(room TEXT,seq INTEGER,parent_head TEXT NOT NULL,head TEXT NOT NULL,stimulus TEXT NOT NULL,timer_changes TEXT NOT NULL,attention TEXT NOT NULL,core TEXT NOT NULL,activity TEXT NOT NULL,authoritative TEXT NOT NULL,PRIMARY KEY(room,seq))")
                self.x("CREATE TABLE operations(op TEXT PRIMARY KEY,caller_hash TEXT NOT NULL,result TEXT NOT NULL)")
                self.x("CREATE TABLE timer_lines(room TEXT,timer_id TEXT,generation INTEGER,scheduled_for INTEGER,state TEXT NOT NULL,PRIMARY KEY(room,timer_id,generation))")
                self.x("CREATE TABLE frames(room TEXT,op TEXT,frame TEXT NOT NULL,PRIMARY KEY(room,op))")
                self.x("CREATE TABLE activations(room TEXT,op TEXT,intent TEXT NOT NULL,decision TEXT NOT NULL,PRIMARY KEY(room,op))")
                # WHERE TRUE disambiguates INSERT ... SELECT ... ON CONFLICT for both parsers.
                self.x("INSERT INTO operations SELECT op,caller_hash,codec FROM legacy_receipts WHERE TRUE ON CONFLICT(op) DO NOTHING")
                self.x("INSERT INTO timer_lines SELECT room,timer_id,generation,scheduled_for,state FROM legacy_timers WHERE TRUE ON CONFLICT(room,timer_id,generation) DO NOTHING")
                self.x("INSERT INTO transitions SELECT room,seq,parent_head,head,stimulus,timer_changes,attention,core,activity,authoritative FROM legacy_transitions WHERE TRUE ON CONFLICT(room,seq) DO NOTHING")
                if fail_v2:
                    raise RuntimeError("injected-v2-migration-interruption")
                self.x("INSERT INTO schema_migrations VALUES(2)")
                self.c.commit()
            except Exception:
                self.c.rollback()
                raise
    def seed(self):
        self.x("INSERT INTO rooms VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)",("counter",GENESIS.seq,GENESIS.transition,GENESIS.core,GENESIS.activity,GENESIS.authoritative,0,100,1,1,1,1,0,0));self.c.commit()
    def snapshot(self,lock=False):
        suffix=" FOR UPDATE" if lock and self.kind=="postgres" else ""
        names=("room","seq","transition_hash","core_hash","activity_hash","authoritative_hash","counter","deadline","healthy","integrity_gen","authority_gen","policy_rev","catching_up","archive")
        r=dict(zip(names,self.x("SELECT "+','.join(names)+" FROM rooms WHERE room=?"+suffix,("counter",)).fetchone()))
        has_v2 = self.x("SELECT 1 FROM schema_migrations WHERE version=2").fetchone()
        timer_table = "timer_lines" if has_v2 else "legacy_timers"
        ts=self.x(f"SELECT timer_id,generation,scheduled_for,state FROM {timer_table} WHERE room=? ORDER BY timer_id,generation",("counter",)).fetchall()
        r["timers"]={(a,b):{"scheduled_for":c,"state":d} for a,b,c,d in ts}
        materialized_core = {"room_status": "archived" if r["archive"] else "active"}
        materialized_activity = {"counter": r["counter"], "deadline": r["deadline"]}
        expected_core = core_digest(materialized_core)
        expected_activity = activity_digest(materialized_activity)
        r["materialization_valid"] = (
            r["core_hash"] == expected_core
            and r["activity_hash"] == expected_activity
            and r["authoritative_hash"] == authoritative_digest(expected_core, expected_activity)
        )
        return r
    def resolve(self,op,caller_hash):
        row=self.x("SELECT caller_hash,result FROM operations WHERE op=?",(op,)).fetchone()
        if not row:return {"outcome":"KnownAbsent"}
        if row[0] != caller_hash:
            return {"outcome":"Conflict"}
        result = json.loads(row[1])
        if result.get("codec_version") == 1:
            # Fresh semantic rendering keeps the old stored codec out of the public seam.
            result = {
                "codec_version": 2,
                "basis_head": result["basis_head"],
                "caller_hash": row[0],
                "kind": "NoChange" if result["disposition"] == "no_change" else "Rejection",
            }
        return {"outcome":"Existing","result":result}
    def begin(self):
        if self.kind=="sqlite":self.c.execute("BEGIN IMMEDIATE")
        else:self.x("BEGIN")
    def commit(self,p:Prepared,inject=None,hide=None):
        # A fast read may be stale. The root lock happens before the decisive re-resolve.
        early=self.resolve(p.op,p.caller_hash)
        if self.kind=="postgres":self.c.commit()
        if early["outcome"]!="KnownAbsent":return early
        try:
            self.begin();r=self.snapshot(lock=True);again=self.resolve(p.op,p.caller_hash)
            if again["outcome"]!="KnownAbsent":self.c.rollback();return again
            if not r["materialization_valid"]:
                self.c.rollback();return {"outcome":"Fault","reason":"materialization-hash-mismatch"}
            b=json.loads(p.bundle)
            for mutation in b["timers"]:
                if mutation.get("miss_precedence") != "before_head":
                    continue
                witness=self.x(
                    "SELECT scheduled_for,state FROM timer_lines WHERE room=? AND timer_id=? AND generation=?",
                    ("counter",mutation["timer_id"],mutation["generation"]),
                ).fetchone()
                if witness != (mutation["expected_scheduled_for"],mutation["expected_state"]):
                    self.c.rollback()
                    return {"outcome":mutation["on_miss"],"reason":"timer-witness-not-current"}
            actual=head_from(r)
            if actual!=p.base:
                self.c.rollback()
                return {"outcome":"Fault","reason":"same-seq-hash-mismatch"} if actual.seq==p.base.seq else {"outcome":"Reprepare","reason":"head-advanced"}
            if (r["healthy"],r["integrity_gen"],r["authority_gen"],r["policy_rev"],r["catching_up"])!=(p.healthy,p.integrity_gen,p.authority_gen,p.policy_rev,p.catching_up):self.c.rollback();return {"outcome":"Fenced","reason":"generation-witness"}
            if r["archive"] and b["type"]!="disposition":
                self.c.rollback();return {"outcome":"Fenced","reason":"archived-core"}
            result={"codec_version":2,"basis_head":p.base.token(),"caller_hash":p.caller_hash,**b["result"]}
            # Only typed bundle installation follows. No kind/reducer decisions are allowed here.
            if b["type"]=="advance":
                t=b["transition"]
                self.x("INSERT INTO transitions VALUES(?,?,?,?,?,?,?,?,?,?)",("counter",t["seq"],canonical(t["parent"]),canonical(t["head"]),t["stimulus"],canonical(t["timer_changes"]),t["attention"],canonical(t["core"]),canonical(t["activity"]),canonical(t["authoritative"])))
                self._hit(inject,"transition")
                self.x("UPDATE rooms SET seq=?,transition_hash=?,core_hash=?,activity_hash=?,authoritative_hash=?,counter=?,deadline=?,archive=? WHERE room=?",(t["head"]["seq"],t["head"]["transition"],t["head"]["core"],t["head"]["activity"],t["head"]["authoritative"],b["room"]["counter"],b["room"]["deadline"],b["room"]["archive"],"counter"))
                self._hit(inject,"room")
            for m in b["timers"]:
                if m["op"]=="insert":self.x("INSERT INTO timer_lines VALUES(?,?,?,?,?)",("counter",m["timer_id"],m["generation"],m["scheduled_for"],m["state"]))
                else:
                    changed=self.x(
                        "UPDATE timer_lines SET state=? WHERE room=? AND timer_id=? AND generation=? AND scheduled_for=? AND state=?",
                        (m["state"],"counter",m["timer_id"],m["generation"],m["expected_scheduled_for"],m["expected_state"]),
                    )
                    if changed.rowcount != 1:
                        raise TimerConditionMiss(m["on_miss"],"timer-witness-not-consumed")
            self._hit(inject,"timer")
            if b["frame"]:self.x("INSERT INTO frames VALUES(?,?,?)",("counter",p.op,canonical(b["frame"])))
            self._hit(inject,"frame")
            if b.get("fence_room"):
                self.x("UPDATE activations SET decision='fenced' WHERE room=? AND decision='accepted'",("counter",))
            if b["activation"]:
                decision="fence-room" if b.get("fence_room") else b["activation"]["decision"]
                self.x("INSERT INTO activations VALUES(?,?,?,?)",("counter",p.op,b["activation"]["intent"],decision))
            self._hit(inject,"activation");self.x("INSERT INTO operations VALUES(?,?,?)",(p.op,p.caller_hash,canonical(result)));self._hit(inject,"operation")
            self.c.commit()
            return {"outcome":"Indeterminate","reason":hide} if hide else {"outcome":"New","result":result}
        except RuntimeError as error:
            self.c.rollback()
            if hide:
                return {"outcome": "Indeterminate", "reason": hide}
            return {"outcome": "RetryableKnownAbsent", "reason": str(error)}
        except TimerConditionMiss as error:
            self.c.rollback()
            return {"outcome":error.outcome,"reason":error.reason}
        except Exception as e:
            self.c.rollback();return {"outcome":"Fault","reason":"constraint-or-corruption","detail":type(e).__name__}
    @staticmethod
    def _hit(inject,name):
        if inject==name:raise RuntimeError("injected-known-abort:"+name)
    def set_witness(self,field,value):
        if self.kind == "postgres":
            self.c.commit()  # close any implicit read transaction before taking the fence
        self.begin()
        self.snapshot(lock=True)  # operational mutations share the Room root fence.
        self.x(f"UPDATE rooms SET {field}=? WHERE room=?",(value,"counter"))
        self.c.commit()
    def durable_fingerprint(self) -> dict[str, Any]:
        """All rows in the atomic Room bundle, normalized for rollback assertions."""
        return {
            "room": self.x(
                "SELECT seq,transition_hash,core_hash,activity_hash,authoritative_hash,counter,deadline,healthy,integrity_gen,authority_gen,policy_rev,catching_up,archive FROM rooms WHERE room=?",
                ("counter",),
            ).fetchone(),
            "transitions": self.x(
                "SELECT seq,parent_head,head,stimulus,timer_changes,attention,core,activity,authoritative FROM transitions WHERE room=? ORDER BY seq",
                ("counter",),
            ).fetchall(),
            "timers": self.x(
                "SELECT timer_id,generation,scheduled_for,state FROM timer_lines WHERE room=? ORDER BY timer_id,generation",
                ("counter",),
            ).fetchall(),
            "frames": self.x(
                "SELECT op,frame FROM frames WHERE room=? ORDER BY op", ("counter",)
            ).fetchall(),
            "activations": self.x(
                "SELECT op,intent,decision FROM activations WHERE room=? ORDER BY op",
                ("counter",),
            ).fetchall(),
            "operations": self.x(
                "SELECT op,caller_hash,result FROM operations ORDER BY op"
            ).fetchall(),
        }
    def replay(self):
        head,counter,deadline=GENESIS,0,GENESIS_ACTIVITY["deadline"]
        timers={}
        sql="SELECT seq,parent_head,head,stimulus,timer_changes,attention,core,activity,authoritative FROM transitions WHERE room=? ORDER BY seq"
        for seq,parent,nh,stim,raw_timers,attention,core,activity,authoritative in self.x(sql,("counter",)).fetchall():
            p,n,s,changes,c,a,au=map(json.loads,(parent,nh,stim,raw_timers,core,activity,authoritative));assert p==head.token()
            next_core_hash = core_digest(c)
            next_activity_hash = activity_digest(a)
            assert au=={"core":next_core_hash,"activity":next_activity_hash}
            next_authoritative_hash = authoritative_digest(next_core_hash, next_activity_hash)
            transition=h({"domain":"CounterRoom/Transition/v1","room":"counter","seq":seq,"prior_transition":head.transition,"stimulus":s,"timer_changes":changes,"attention":attention,"core":next_core_hash,"activity":next_activity_hash,"authoritative":next_authoritative_hash})
            expect=Head(seq,transition,next_core_hash,next_activity_hash,next_authoritative_hash);assert expect.token()==n
            for m in changes:
                key=(m["timer_id"],m["generation"])
                if m["op"]=="insert":timers[key]={"scheduled_for":m["scheduled_for"],"state":m["state"]}
                else:timers[key]={**timers[key],"state":m["state"]}
            head,counter,deadline=expect,a["counter"],a["deadline"]
        return {"head":head.token(),"counter":counter,"deadline":deadline,"core":{"room_status":"archived" if head.core==core_digest({"room_status":"archived"}) else "active"},"timers":timers}


TABLES = (
    "activations",
    "frames",
    "timer_lines",
    "operations",
    "transitions",
    "legacy_transitions",
    "legacy_timers",
    "legacy_receipts",
    "rooms",
    "schema_migrations",
)


def drop_all(kind: str, connect: Callable[[], Any]) -> None:
    connection = connect()
    try:
        for table in TABLES:
            suffix = " CASCADE" if kind == "postgres" else ""
            connection.cursor().execute(f"DROP TABLE IF EXISTS {table}{suffix}")
        connection.commit()
    finally:
        connection.close()


LEGACY_V1 = {
    # Frozen bytes: this fixture must not be regenerated through current prepare()/canonical().
    "caller_hash": "f67e7b64dc0971615bba79b3da070c083b8a30ce264e706e8777f5501c40a49c",
    "parent": '{"activity":"d759c032d38b08e70f3e73284c354595b125ae7f6fec1d3cdc5cfd2e48189964","authoritative":"1b3b1bb3868a6180844dfd6e598ee689dd3ba6d9d5f488838f0e419783daa0d6","core":"e04a4a90389c4d9b6e5beb264727f98397adfd3d2721d243406ca76ac086552a","seq":0,"transition":"a44a8ec7434ff19224d9c8e451b7d86b93503bc6397647db24f1f2cd8a064210"}',
    "head": '{"activity":"0e151107a6473eb0909dcc9fb193e44fd12b46fa0d7ff2d20536f94f840b7397","authoritative":"b20bad136cc91ab8726526671753a844ea7b9b0467b0b869a0bebe69ebb07933","core":"e04a4a90389c4d9b6e5beb264727f98397adfd3d2721d243406ca76ac086552a","seq":1,"transition":"6096ecb855f6b914d27810b1fd9532581f4824d6b8afae2e7163e20e3db7911e"}',
    "stimulus": '{"participant_admitted_at":0,"request":{"delta":7,"kind":"advance"}}',
    "timer_changes": "[]",
    "attention": "counter-attention",
    "core": '{"room_status":"active"}',
    "activity": '{"counter":7,"deadline":100}',
    "authoritative": '{"activity":"0e151107a6473eb0909dcc9fb193e44fd12b46fa0d7ff2d20536f94f840b7397","core":"e04a4a90389c4d9b6e5beb264727f98397adfd3d2721d243406ca76ac086552a"}',
    "receipt": '{"basis_head":{"activity":"d759c032d38b08e70f3e73284c354595b125ae7f6fec1d3cdc5cfd2e48189964","authoritative":"1b3b1bb3868a6180844dfd6e598ee689dd3ba6d9d5f488838f0e419783daa0d6","core":"e04a4a90389c4d9b6e5beb264727f98397adfd3d2721d243406ca76ac086552a","seq":0,"transition":"a44a8ec7434ff19224d9c8e451b7d86b93503bc6397647db24f1f2cd8a064210"},"codec_version":1,"disposition":"no_change"}',
}


def migration_probe(kind: str, connect: Callable[[], Any]) -> dict[str, Any]:
    """Prove an interrupted v2 is not ready, then preserve v1 semantic lineage."""
    drop_all(kind, connect)
    connection = connect()
    try:
        v1 = DB(connection, kind, target_version=1)
        v1.seed()
        assert canonical(GENESIS.token()) == LEGACY_V1["parent"]
        expected_head = json.loads(LEGACY_V1["head"])
        v1.x(
            "INSERT INTO legacy_transitions VALUES(?,?,?,?,?,?,?,?,?,?)",
            (
                "counter",
                1,
                LEGACY_V1["parent"],
                LEGACY_V1["head"],
                LEGACY_V1["stimulus"],
                LEGACY_V1["timer_changes"],
                LEGACY_V1["attention"],
                LEGACY_V1["core"],
                LEGACY_V1["activity"],
                LEGACY_V1["authoritative"],
            ),
        )
        v1.x(
            "UPDATE rooms SET seq=?,transition_hash=?,core_hash=?,activity_hash=?,authoritative_hash=?,counter=? WHERE room=?",
            (
                expected_head["seq"],
                expected_head["transition"],
                expected_head["core"],
                expected_head["activity"],
                expected_head["authoritative"],
                7,
                "counter",
            ),
        )
        legacy_caller_hash = LEGACY_V1["caller_hash"]
        v1.x(
            "INSERT INTO legacy_receipts VALUES(?,?,?)",
            (
                "legacy-receipt",
                legacy_caller_hash,
                LEGACY_V1["receipt"],
            ),
        )
        v1.x(
            "INSERT INTO legacy_timers VALUES(?,?,?,?,?)",
            ("counter", "legacy-wake", 41, 77, "scheduled"),
        )
        connection.commit()

        try:
            DB(connection, kind, fail_v2=True)
        except RuntimeError as error:
            assert str(error) == "injected-v2-migration-interruption"
        else:
            raise AssertionError("v2 interruption was not injected")

        versions = [row[0] for row in v1.x("SELECT version FROM schema_migrations ORDER BY version").fetchall()]
        assert versions == [1]
        try:
            v1.x("SELECT 1 FROM operations")
        except Exception:
            connection.rollback()
        else:
            raise AssertionError("partial v2 table survived the interrupted migration")

        upgraded = DB(connection, kind)
        existing = upgraded.resolve("legacy-receipt", legacy_caller_hash)
        conflict = upgraded.resolve("legacy-receipt", h({"kind": "changed"}))
        replayed = upgraded.replay()
        snapshot = upgraded.snapshot()
        assert existing == {
            "outcome": "Existing",
            "result": {
                "codec_version": 2,
                "basis_head": json.loads(LEGACY_V1["parent"]),
                "caller_hash": legacy_caller_hash,
                "kind": "NoChange",
            },
        }
        assert conflict == {"outcome": "Conflict"}
        assert replayed["head"] == expected_head and replayed["counter"] == 7
        assert snapshot["timers"] == {
            ("legacy-wake", 41): {"scheduled_for": 77, "state": "scheduled"}
        }
        final_versions = [row[0] for row in upgraded.x("SELECT version FROM schema_migrations ORDER BY version").fetchall()]
        assert final_versions == [1, 2]
        return {
            "interrupted_versions": versions,
            "partial_v2_visible": False,
            "final_versions": final_versions,
            "legacy_receipt": existing,
            "changed_hash": conflict,
            "timer_retained": {
                "timer_id": "legacy-wake",
                "generation": 41,
                "scheduled_for": 77,
                "state": "scheduled",
            },
            "replay": {"head": replayed["head"], "counter": replayed["counter"]},
        }
    finally:
        connection.close()


def stable_overdue(snapshot: dict[str, Any], now: int) -> list[dict[str, Any]]:
    overdue = [
        {
            "timer_id": timer_id,
            "generation": generation,
            "scheduled_for": timer["scheduled_for"],
        }
        for (timer_id, generation), timer in snapshot["timers"].items()
        if timer["state"] == "scheduled" and timer["scheduled_for"] <= now
    ]
    return sorted(
        overdue,
        key=lambda timer: (
            timer["scheduled_for"],
            timer["timer_id"],
            timer["generation"],
        ),
    )

def scenario(kind:str,connect:Callable[[],Any]):
    c=connect();db=DB(c,kind);db.seed();clock=Clock();log=[]
    def go(name,request,admit=None,**kw):
        caller_hash=h(request)
        resolved=db.resolve(name,caller_hash)
        if resolved["outcome"]!="KnownAbsent":
            p=None;out=resolved
        else:
            p=prepare(db.snapshot(),name,request,clock.now if admit is None else admit)
            out=p if isinstance(p,dict) else db.commit(p,**kw)
        log.append({"step":name,"caller_hash":caller_hash,"outcome":out});return p,out
    initial_basis=head_from(db.snapshot())
    initial_prepared,initial_outcome=go("advance",{"kind":"advance","delta":2})
    assert isinstance(initial_prepared,Prepared) and initial_outcome["outcome"]=="New"
    assert initial_outcome["result"]["codec_version"]==2 and initial_outcome["result"]["basis_head"]==initial_basis.token()
    assert db.resolve(initial_prepared.op,initial_prepared.caller_hash)=={"outcome":"Existing","result":initial_outcome["result"]}
    for x in ("transition","room","timer","frame","activation","operation"):
        before=db.durable_fingerprint()
        p,o=go("rollback-"+x,{"kind":"schedule","timer_id":"r"+x,"generation":1,"scheduled_for":10},inject=x)
        after=db.durable_fingerprint()
        assert isinstance(p,Prepared) and o["outcome"]=="RetryableKnownAbsent"
        assert db.resolve(p.op,p.caller_hash)["outcome"]=="KnownAbsent" and after==before
    p,_=go("lost-committed",{"kind":"advance","delta":1},hide="reply-lost-after-commit");log.append({"step":"resolve-lost-committed","outcome":db.resolve(p.op,p.caller_hash)})
    p,_=go("lost-absent",{"kind":"advance","delta":1},inject="operation",hide="reply-lost-before-commit");log.append({"step":"resolve-lost-absent","outcome":db.resolve(p.op,p.caller_hash)})
    installed,_=go("postcommit-install-failure",{"kind":"advance","delta":1})
    reload_connection=connect();reloaded=DB(reload_connection,kind)
    reload_resolution=reloaded.resolve(installed.op,installed.caller_hash)
    reload_replay=reloaded.replay();reload_connection.close()
    assert reload_resolution["outcome"]=="Existing" and reload_replay["head"]==head_from(db.snapshot()).token()
    log.append({"step":"postcommit-reload","outcome":{"resolution":reload_resolution,"replay_head":reload_replay["head"]}})
    p,_=go("same",{"kind":"advance","delta":1});log.append({"step":"same-existing","outcome":db.commit(p)});q=prepare(db.snapshot(),"same",{"kind":"advance","delta":9},clock.now);log.append({"step":"same-conflict","outcome":db.commit(q)})
    # disposition first leaves Head; later advance still commits. Advance first makes disposition Reprepare.
    a=prepare(db.snapshot(),"order-a",{"kind":"advance","delta":1},clock.now);r=prepare(db.snapshot(),"order-r",{"kind":"receipt"},clock.now);log += [{"step":"receipt-first","outcome":db.commit(r)},{"step":"advance-after-receipt","outcome":db.commit(a)}]
    a=prepare(db.snapshot(),"order-a2",{"kind":"advance","delta":1},clock.now);r=prepare(db.snapshot(),"order-r2",{"kind":"receipt"},clock.now);log += [{"step":"advance-first","outcome":db.commit(a)},{"step":"receipt-stale","outcome":db.commit(r)}]
    go("timer-line",{"kind":"schedule","timer_id":"wake","generation":1,"scheduled_for":10})
    clock.now=9
    before_d=stable_overdue(db.snapshot(),clock.now)
    assert not any(timer["timer_id"]=="wake" for timer in before_d)
    log.append({"step":"timer-before-D-not-submitted","outcome":{"outcome":"NotSubmitted"}})
    clock.now=10
    timer_request={"kind":"fire","timer_id":"wake","generation":1,"scheduled_for":10}
    p,o=go("timer:counter:wake:1",timer_request)
    assert isinstance(p,Prepared) and o["outcome"]=="New"
    _,fresh_retry=go("timer:counter:wake:1",timer_request)
    assert fresh_retry["outcome"]=="Existing"
    timer_restart_connection=connect();timer_restart_db=DB(timer_restart_connection,kind)
    timer_restart_resolution=timer_restart_db.resolve("timer:counter:wake:1",h(timer_request));timer_restart_connection.close()
    assert timer_restart_resolution["outcome"]=="Existing"
    log.append({"step":"fired-retry-after-restart","outcome":timer_restart_resolution})
    _,changed_retry=go("timer:counter:wake:1",{"kind":"fire","timer_id":"wake","generation":1,"scheduled_for":11})
    assert changed_retry=={"outcome":"Conflict"}
    _,generation_reuse=go("timer-generation-reuse",{"kind":"schedule","timer_id":"wake","generation":1,"scheduled_for":11})
    _,missing_cancel=go("timer-cancel-missing",{"kind":"cancel","timer_id":"wake","generation":99})
    _,not_forward=go("timer-not-forward",{"kind":"schedule","timer_id":"past","generation":1,"scheduled_for":10})
    assert generation_reuse=={"outcome":"Fault","reason":"timer-generation-not-next"}
    assert missing_cancel=={"outcome":"Fault","reason":"invalid-pack-timer-cancel"}
    assert not_forward=={"outcome":"Fault","reason":"timer-not-strictly-forward"}
    go("witness-delete-schedule",{"kind":"schedule","timer_id":"witness-delete","generation":1,"scheduled_for":25})
    witness_candidate=prepare(db.snapshot(),"timer:counter:witness-delete:1",{"kind":"fire","timer_id":"witness-delete","generation":1,"scheduled_for":25},25)
    assert isinstance(witness_candidate,Prepared)
    witness_head=head_from(db.snapshot())
    db.x("DELETE FROM timer_lines WHERE room=? AND timer_id=? AND generation=?",("counter","witness-delete",1));db.c.commit()
    witness_missing=db.commit(witness_candidate)
    assert witness_missing["outcome"]=="NotApplicable" and head_from(db.snapshot())==witness_head
    assert db.resolve(witness_candidate.op,witness_candidate.caller_hash)["outcome"]=="KnownAbsent"
    db.x("INSERT INTO timer_lines VALUES(?,?,?,?,?)",("counter","witness-delete",1,25,"scheduled"));db.c.commit()
    log.append({"step":"timer-witness-deleted-after-prepare","outcome":witness_missing})
    go("witness-delete-cleanup",{"kind":"cancel","timer_id":"witness-delete","generation":1})
    go("reschedule-g1",{"kind":"schedule","timer_id":"reschedule","generation":1,"scheduled_for":20})
    _,duplicate_live=go("duplicate-live-schedule",{"kind":"schedule","timer_id":"reschedule","generation":1,"scheduled_for":20})
    assert duplicate_live["outcome"]=="Fault"
    before_reschedule=db.snapshot()["seq"]
    _,reschedule_outcome=go("reschedule-to-g2",{"kind":"reschedule","timer_id":"reschedule","generation":1,"next_generation":2,"scheduled_for":30})
    after_reschedule=db.snapshot()
    assert reschedule_outcome["outcome"]=="New" and after_reschedule["seq"]==before_reschedule+1
    assert after_reschedule["timers"][("reschedule",1)]["state"]=="cancelled"
    assert after_reschedule["timers"][("reschedule",2)]=={"scheduled_for":30,"state":"scheduled"}
    _,skipped_generation=go("reschedule-skip-generation",{"kind":"reschedule","timer_id":"reschedule","generation":2,"next_generation":4,"scheduled_for":40})
    assert skipped_generation=={"outcome":"Fault","reason":"timer-generation-not-next"}
    _,stale_cancel=go("cancel-stale-generation",{"kind":"cancel","timer_id":"reschedule","generation":1})
    assert stale_cancel["outcome"]=="Fault"
    clock.now=30
    _,old_generation_fire=go("timer:counter:reschedule:1",{"kind":"fire","timer_id":"reschedule","generation":1,"scheduled_for":20})
    _,new_generation_fire=go("timer:counter:reschedule:2",{"kind":"fire","timer_id":"reschedule","generation":2,"scheduled_for":30})
    assert old_generation_fire["outcome"]=="NotApplicable"
    assert new_generation_fire["outcome"]=="New" and new_generation_fire["result"]["kind"]=="Advance"
    head_timer_request={"kind":"fire","timer_id":"head-wake","generation":1,"scheduled_for":35}
    go("head-wake-schedule",{"kind":"schedule","timer_id":"head-wake","generation":1,"scheduled_for":35})
    clock.now=35
    head_timer=prepare(db.snapshot(),"timer:counter:head-wake:1",head_timer_request,clock.now)
    assert isinstance(head_timer,Prepared)
    go("advance-before-prepared-timer",{"kind":"advance","delta":1})
    timer_head_stale=db.commit(head_timer)
    assert timer_head_stale["outcome"]=="Reprepare"
    log.append({"step":"timer-head-reprepare","outcome":timer_head_stale})
    reprepared_timer=prepare(db.snapshot(),"timer:counter:head-wake:1",head_timer_request,clock.now)
    assert isinstance(reprepared_timer,Prepared) and reprepared_timer.caller_hash==head_timer.caller_hash
    timer_head_new=db.commit(reprepared_timer)
    assert timer_head_new["outcome"]=="New" and timer_head_new["result"]["kind"]=="Advance"
    log.append({"step":"timer-head-reprepared-same-identity","outcome":timer_head_new})
    for field in ("policy_rev","authority_gen","integrity_gen"):
        p=prepare(db.snapshot(),"race-"+field,{"kind":"advance","delta":1},clock.now);db.set_witness(field,db.snapshot()[field]+1);log.append({"step":"race-"+field,"outcome":db.commit(p)})
    health_candidate = prepare(db.snapshot(), "race-healthy", {"kind": "advance", "delta": 1}, clock.now)
    assert isinstance(health_candidate, Prepared)
    db.set_witness("healthy", 0)
    log.append({"step": "race-healthy", "outcome": db.commit(health_candidate)})
    _,existing_while_unhealthy=go("timer:counter:wake:1",timer_request)
    assert existing_while_unhealthy["outcome"]=="Existing"
    db.set_witness("integrity_gen", db.snapshot()["integrity_gen"] + 1)
    db.set_witness("healthy", 1)

    catching_up_candidate=prepare(db.snapshot(),"race-catching-up",{"kind":"advance","delta":1},clock.now)
    assert isinstance(catching_up_candidate,Prepared)
    db.set_witness("catching_up",1)
    catching_up_race=db.commit(catching_up_candidate)
    assert catching_up_race["outcome"]=="Fenced"
    log.append({"step":"race-catching-up","outcome":catching_up_race})
    db.set_witness("catching_up",0)

    db.set_witness("catching_up", 1)
    catching_up = prepare(db.snapshot(), "catching-up", {"kind": "advance", "delta": 1}, clock.now)
    assert catching_up == {"outcome": "Reprepare", "reason": "CatchingUp"}
    log.append({"step": "catching-up", "outcome": catching_up})
    db.set_witness("catching_up", 0)

    go("overdue-alpha", {"kind": "schedule", "timer_id": "alpha", "generation": 1, "scheduled_for": 40})
    go("overdue-beta", {"kind": "schedule", "timer_id": "beta", "generation": 1, "scheduled_for": 39})
    clock.now = 40
    overdue = stable_overdue(db.snapshot(), clock.now)
    assert [timer["timer_id"] for timer in overdue] == ["beta", "alpha"]
    log.append({"step": "stable-overdue-order", "outcome": overdue})
    clock.now=99
    d_minus_one=prepare(db.snapshot(),"action-admitted-D-1",{"kind":"advance","delta":3},clock.now)
    assert isinstance(d_minus_one,Prepared)
    clock.now=100
    d_minus_one_outcome=db.commit(d_minus_one)
    assert d_minus_one_outcome["outcome"]=="New" and d_minus_one_outcome["result"]["kind"]=="Advance"
    log.append({"step":"action-admitted-D-1-commits-at-D","outcome":d_minus_one_outcome})
    _,deadline_rejection=go("deadline-D-rejection",{"kind":"advance","delta":2})
    assert deadline_rejection["outcome"]=="New" and deadline_rejection["result"]["kind"]=="Rejection"
    corrupted=prepare(db.snapshot(),"same-seq-corruption",{"kind":"advance","delta":1},clock.now)
    original=db.snapshot()["transition_hash"]
    db.x("UPDATE rooms SET transition_hash=? WHERE room=?",("corrupt-same-seq","counter"));db.c.commit()
    corruption_outcome=db.commit(corrupted)
    assert corruption_outcome=={"outcome":"Fault","reason":"same-seq-hash-mismatch"}
    log.append({"step":"same-seq-corruption","outcome":corruption_outcome})
    db.x("UPDATE rooms SET transition_hash=? WHERE room=?",(original,"counter"));db.c.commit()
    go("archive-timer",{"kind":"schedule","timer_id":"archive-wake","generation":1,"scheduled_for":150})
    archived=prepare(db.snapshot(),"archive",{"kind":"archive"},clock.now)
    stale_action=prepare(db.snapshot(),"after-archive-action",{"kind":"advance","delta":1},clock.now)
    stale_timer=prepare(db.snapshot(),"after-archive-timer",{"kind":"fire","timer_id":"archive-wake","generation":1,"scheduled_for":150},150)
    archive_outcome=db.commit(archived);log.append({"step":"archive-first","outcome":archive_outcome})
    action_outcome=db.commit(stale_action);log.append({"step":"archive-vs-action","outcome":action_outcome})
    timer_outcome=db.commit(stale_timer);log.append({"step":"archive-vs-timer","outcome":timer_outcome})
    assert archive_outcome["outcome"]=="New" and action_outcome["outcome"]=="Reprepare" and timer_outcome["outcome"]=="NotApplicable"
    _,archived_rejection=go("after-archive-action",{"kind":"advance","delta":1})
    assert archived_rejection["outcome"]=="New" and archived_rejection["result"]["reason"]=="room-archived"
    _,archive_existing=go("archive",{"kind":"archive"})
    _,fired_existing=go("timer:counter:wake:1",timer_request)
    assert archive_existing["outcome"]=="Existing" and fired_existing["outcome"]=="Existing"
    end=db.snapshot();replayed=db.replay()
    activation_states=db.x("SELECT decision FROM activations WHERE room=? ORDER BY op",("counter",)).fetchall()
    assert all(timer["state"]!="scheduled" for timer in end["timers"].values())
    assert any(state[0]=="fence-room" for state in activation_states) and any(state[0]=="fenced" for state in activation_states)
    assert replayed["head"]==head_from(end).token() and replayed["counter"]==end["counter"] and replayed["deadline"]==end["deadline"] and replayed["core"]=={"room_status":"archived"} and replayed["timers"]==end["timers"]
    timer_list=sorted([{**v,"timer_id":k[0],"generation":k[1]} for k,v in end["timers"].items()],key=lambda x:(x["scheduled_for"],x["timer_id"],x["generation"]))
    return {"events":log,"replay":{**replayed,"timers":timer_list},"head":head_from(end).token(),"counter":end["counter"],"timers":timer_list}

def contend(kind,connect):
    # Real independent connections enter simultaneously; arbitrary distinct-Advance winner is normalized away.
    def fresh():
        drop_all(kind, connect)
        root = connect()
        DB(root, kind).seed()
        root.close()
        seed = connect()
        base = DB(seed, kind).snapshot()
        seed.close()
        return base

    def pair(
        make: Callable[[dict[str, Any]], tuple[Prepared, Prepared]],
        *,
        legal_outcomes: set[tuple[str, str]],
        legal_transition_counts: set[int],
        legal_operation_counts: set[int],
        legal_counters: set[int],
    ) -> dict[str, Any]:
        base = fresh()
        first, second = make(base)
        barrier = threading.Barrier(3)
        outcomes: list[dict[str, Any]] = []

        def run(prepared: Prepared) -> None:
            connection = connect()
            try:
                database = DB(connection, kind)
                barrier.wait()
                outcomes.append(database.commit(prepared))
            finally:
                connection.close()

        threads = [
            threading.Thread(target=run, args=(first,)),
            threading.Thread(target=run, args=(second,)),
        ]
        for thread in threads:
            thread.start()
        barrier.wait()
        for thread in threads:
            thread.join()

        normalized_outcomes = tuple(sorted(outcome["outcome"] for outcome in outcomes))
        assert normalized_outcomes in legal_outcomes
        check = connect()
        try:
            database = DB(check, kind)
            state = database.snapshot()
            transition_count = database.x(
                "SELECT COUNT(*) FROM transitions WHERE room=?", ("counter",)
            ).fetchone()[0]
            operation_count = database.x(
                "SELECT COUNT(*) FROM operations", ()
            ).fetchone()[0]
        finally:
            check.close()
        assert transition_count in legal_transition_counts
        assert operation_count in legal_operation_counts
        assert state["counter"] in legal_counters
        return {
            "legal_serialization": True,
            "outcomes": list(normalized_outcomes) if len(legal_outcomes) == 1 else "one-of-legal-orders",
            "durable_invariants": True,
        }

    def control_race(field: str) -> dict[str, Any]:
        base = fresh()
        candidate = prepare(base, "control-race", {"kind": "advance", "delta": 1}, 0)
        assert isinstance(candidate, Prepared)
        barrier = threading.Barrier(3)
        commit_outcomes: list[dict[str, Any]] = []

        def commit_candidate() -> None:
            connection = connect()
            try:
                database = DB(connection, kind)
                barrier.wait()
                commit_outcomes.append(database.commit(candidate))
            finally:
                connection.close()

        def change_control() -> None:
            connection = connect()
            try:
                database = DB(connection, kind)
                barrier.wait()
                database.set_witness(field, base[field] + 1)
            finally:
                connection.close()

        threads = [threading.Thread(target=commit_candidate), threading.Thread(target=change_control)]
        for thread in threads:
            thread.start()
        barrier.wait()
        for thread in threads:
            thread.join()

        outcome = commit_outcomes[0]["outcome"]
        assert outcome in {"New", "Fenced"}
        check = connect()
        try:
            database = DB(check, kind)
            state = database.snapshot()
            transition_count = database.x(
                "SELECT COUNT(*) FROM transitions WHERE room=?", ("counter",)
            ).fetchone()[0]
        finally:
            check.close()
        assert state[field] == base[field] + 1
        assert (outcome, transition_count, state["counter"]) in {
            ("New", 1, 1),
            ("Fenced", 0, 0),
        }
        return {"legal_serialization": True, "commit_outcome": "one-of-legal-orders"}

    return {
        "distinct_advances": pair(
            lambda base: (
                prepare(base, "a", {"kind": "advance", "delta": 1}, 0),
                prepare(base, "b", {"kind": "advance", "delta": 2}, 0),
            ),
            legal_outcomes={("New", "Reprepare")},
            legal_transition_counts={1},
            legal_operation_counts={1},
            legal_counters={1, 2},
        ),
        "same_identity": pair(
            lambda base: (
                prepare(base, "id", {"kind": "advance", "delta": 1}, 0),
                prepare(base, "id", {"kind": "advance", "delta": 1}, 0),
            ),
            legal_outcomes={("Existing", "New")},
            legal_transition_counts={1},
            legal_operation_counts={1},
            legal_counters={1},
        ),
        "same_identity_changed_hash": pair(
            lambda base: (
                prepare(base, "id", {"kind": "advance", "delta": 1}, 0),
                prepare(base, "id", {"kind": "advance", "delta": 2}, 0),
            ),
            legal_outcomes={("Conflict", "New")},
            legal_transition_counts={1},
            legal_operation_counts={1},
            legal_counters={1, 2},
        ),
        "two_receipts": pair(
            lambda base: (
                prepare(base, "receipt-a", {"kind": "receipt"}, 0),
                prepare(base, "receipt-b", {"kind": "receipt"}, 0),
            ),
            legal_outcomes={("New", "New")},
            legal_transition_counts={0},
            legal_operation_counts={2},
            legal_counters={0},
        ),
        "advance_vs_receipt": pair(
            lambda base: (
                prepare(base, "advance", {"kind": "advance", "delta": 1}, 0),
                prepare(base, "receipt", {"kind": "receipt"}, 0),
            ),
            legal_outcomes={("New", "New"), ("New", "Reprepare")},
            legal_transition_counts={1},
            legal_operation_counts={1, 2},
            legal_counters={1},
        ),
        "authority_generation_race": control_race("authority_gen"),
    }

def port():
    s=socket.socket();s.bind(("127.0.0.1",0));n=s.getsockname()[1];s.close();return n
@contextlib.contextmanager
def pg_cluster():
    if not psycopg2 or not shutil.which("initdb") or not shutil.which("pg_ctl"):raise RuntimeError("psycopg2/initdb/pg_ctl unavailable")
    with tempfile.TemporaryDirectory(prefix="room-commit-contract-pg-") as t:
        d=Path(t)/"data";p=port();subprocess.run([shutil.which("initdb"),"-D",str(d),"--auth=trust","--no-locale"],check=True,stdout=subprocess.DEVNULL);subprocess.run([shutil.which("pg_ctl"),"-D",str(d),"-o",f"-h 127.0.0.1 -p {p}","-w","start"],check=True,stdout=subprocess.DEVNULL)
        try:yield lambda:psycopg2.connect(host="127.0.0.1",port=p,user=os.environ.get("USER","postgres"),dbname="postgres")
        finally:subprocess.run([shutil.which("pg_ctl"),"-D",str(d),"-m","immediate","-w","stop"],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
def main():
    ap=argparse.ArgumentParser();ap.add_argument("--sqlite-only",action="store_true");a=ap.parse_args()
    with tempfile.TemporaryDirectory(prefix="room-commit-contract-sqlite-") as td:
        migration_path = str(Path(td) / "migration.sqlite")
        scenario_path = str(Path(td) / "scenario.sqlite")
        sqlite_migration = lambda: sqlite3.connect(migration_path, timeout=5, check_same_thread=False)
        sq=lambda:sqlite3.connect(scenario_path,timeout=5,check_same_thread=False)
        sqlite={
            "migration": migration_probe("sqlite", sqlite_migration),
            "scenario":scenario("sqlite",sq),
            "contention":contend("sqlite",sq),
        }
    if a.sqlite_only: print(json.dumps({"sqlite":sqlite},indent=2,sort_keys=True));print("PASS: SQLite self-check completed.");return
    with pg_cluster() as pg:
        postgres_migration = migration_probe("postgres", pg)
        drop_all("postgres", pg)
        postgres={
            "migration": postgres_migration,
            "scenario":scenario("postgres",pg),
            "contention":contend("postgres",pg),
        }
    assert sqlite==postgres,(sqlite,postgres);print(json.dumps({"sqlite":sqlite,"postgres":postgres},indent=2,sort_keys=True));print("PASS: normalized SQLite/PostgreSQL semantic transcripts match.")
if __name__=="__main__":main()
