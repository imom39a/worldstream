#!/usr/bin/env python3
"""Measure actual canonical backend cells in isolated child processes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import signal
import sqlite3
import subprocess
import sys
import time
import urllib.parse

ROOT = Path(__file__).resolve().parent.parent
ROOM = '01ARZ3NDEKTSV4RRFFQ69G5FAV'
RUST_SOURCE = ROOT / 'crates/worldstream-conformance/examples/measure_backend_stream_scaling.rs'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def captured(argv, env=None, input=None):
    result = subprocess.run(argv, cwd=ROOT, env=env, input=input, capture_output=True, text=True)
    if result.returncode:
        # Do not copy provider credentials or connection strings to a report.
        raise RuntimeError('subprocess failed: ' + Path(argv[0]).name)
    return result.stdout.strip()


def dsn_fields(dsn):
    if '://' in dsn:
        uri = urllib.parse.urlsplit(dsn)
        fields = dict(urllib.parse.parse_qsl(uri.query))
        fields.update(host=uri.hostname or '', port=str(uri.port or 5432), user=urllib.parse.unquote(uri.username or ''), dbname=uri.path.lstrip('/'))
        if uri.password is not None:
            fields['password'] = urllib.parse.unquote(uri.password)
        return fields
    return dict(item.split('=', 1) for item in shlex.split(dsn))


def replace_database(dsn, database):
    fields = dsn_fields(dsn)
    fields['dbname'] = database
    return ' '.join(key + "='" + str(value).replace('\\', '\\\\').replace("'", "\\'") + "'" for key, value in fields.items())


def pg_environment(dsn):
    result = os.environ.copy()
    for key, value in dsn_fields(dsn).items():
        mapping = {'host':'PGHOST', 'port':'PGPORT', 'user':'PGUSER', 'password':'PGPASSWORD', 'dbname':'PGDATABASE', 'sslmode':'PGSSLMODE'}
        if key in mapping:
            result[mapping[key]] = value
    return result


def psql(sql, dsn=None):
    dsn = dsn or os.environ['WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN']
    return captured(['psql', '--no-psqlrc', '--quiet', '--tuples-only', '--no-align', '--set', 'ON_ERROR_STOP=1'], env=pg_environment(dsn), input=sql)


def probe(args):
    sqlite = args.backend == 'sqlite'
    prefix = '' if sqlite else 'worldstream_'
    tables = {'transitions': prefix+'transitions', 'snapshots':prefix+'room_snapshots', 'witness':prefix+'room_snapshot_operational_witnesses_v3', 'witness2':prefix+'room_snapshot_operational_witnesses_v2', 'schedule':prefix+'room_snapshot_schedules', 'genesis':'room_genesis' if sqlite else 'worldstream_genesis', 'materializations':'room_materializations' if sqlite else 'worldstream_materializations', 'receipts':'semantic_receipts' if sqlite else 'worldstream_semantic_receipts', 'frames':'observation_frames' if sqlite else 'worldstream_frames'}
    conn = sqlite3.connect(args.cell_dir / 'room.db', timeout=60) if sqlite else None
    if conn:
        conn.execute('PRAGMA foreign_keys=ON')
    def sql(query):
        if sqlite:
            conn.executescript(query)
            conn.commit()
            return ''
        return psql(query)
    if args.probe == 'records':
        query = f"SELECT transition_bytes FROM {tables['transitions']} WHERE room_id='{ROOM}' ORDER BY room_seq"
        if sqlite:
            for row in conn.execute(query):
                print(row[0].hex())
        else:
            query = f"COPY (SELECT encode(transition_bytes,'hex') FROM {tables['transitions']} WHERE room_id='{ROOM}' ORDER BY room_seq) TO STDOUT;"
            proc = subprocess.run(['psql', '--no-psqlrc', '--quiet', '--set', 'ON_ERROR_STOP=1'], env=pg_environment(os.environ['WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN']), input=query, text=True)
            return proc.returncode
        return 0
    if args.probe == 'checkpoint-due':
        sql(f"UPDATE {tables['schedule']} SET transitions_since_snapshot=249 WHERE room_id='{ROOM}';")
    elif args.probe == 'fence':
        table, column = ('room_integrity','generation') if sqlite else ('worldstream_room_roots','integrity_generation')
        sql(f"UPDATE {table} SET {column}={column}+1 WHERE room_id='{ROOM}';")
    elif args.probe == 'tail':
        sql(f"DELETE FROM {tables['snapshots']} WHERE room_id='{ROOM}' AND room_seq=(SELECT max(room_seq) FROM {tables['snapshots']} WHERE room_id='{ROOM}');")
    elif args.probe == 'invalid':
        for table in [tables['witness'], tables['witness2']]:
            zero = "X'"+'00'*32+"'" if sqlite else "decode('"+'00'*32+"','hex')"
            sql(f"UPDATE {table} SET witness_hash={zero} WHERE room_id='{ROOM}';")
    elif args.probe == 'absent':
        sql(f"DELETE FROM {tables['snapshots']} WHERE room_id='{ROOM}';")
    elif args.probe == 'metrics':
        length = 'length' if sqlite else 'octet_length'
        scalar = lambda q: conn.execute(q).fetchone()[0] if sqlite else json.loads(psql('SELECT to_json(('+q+'));'))
        report = {}
        for label, table, column in [('transition',tables['transitions'],'transition_bytes'), ('genesis',tables['genesis'],'genesis_bytes'), ('frame',tables['frames'],'payload_bytes'), ('checkpoint_witness_v3',tables['witness'],'witness_bytes'), ('checkpoint_witness_v2',tables['witness2'],'witness_bytes')]:
            report[label+'_count'] = scalar(f"SELECT count(*) FROM {table} WHERE room_id='{ROOM}'")
            report[label+'_bytes'] = scalar(f"SELECT coalesce(sum({length}({column})),0) FROM {table} WHERE room_id='{ROOM}'")
        report['checkpoint_count'] = scalar(f"SELECT count(*) FROM {tables['snapshots']} WHERE room_id='{ROOM}'")
        report['checkpoint_newest_seq'] = scalar(f"SELECT max(room_seq) FROM {tables['snapshots']} WHERE room_id='{ROOM}'")
        report['checkpoint_bytes'] = scalar(f"SELECT coalesce(sum({length}(complete_head_bytes)+{length}(core_state_bytes)+{length}(activity_state_bytes)),0) FROM {tables['snapshots']} WHERE room_id='{ROOM}'")
        for name in ['core_state_bytes','activity_state_bytes']:
            report['current_'+name] = scalar(f"SELECT {length}({name}) FROM {tables['materializations']} WHERE room_id='{ROOM}'")
        report['receipt_count'] = scalar('SELECT count(*) FROM '+tables['receipts'])
        if sqlite:
            report['sqlite_version'] = sqlite3.sqlite_version
            report['sqlite_settings'] = {key:conn.execute('PRAGMA '+key).fetchone()[0] for key in ['page_size','page_count','freelist_count','journal_mode','synchronous','wal_autocheckpoint']}
            report['sqlite_settings_note'] = 'Probe connection settings; production bundled identity is separately reported by Rust. Production default writer synchronous is FULL.'
            report['files_before_probe_checkpoint'] = {suffix: (args.cell_dir / ('room.db'+suffix)).stat().st_size if (args.cell_dir / ('room.db'+suffix)).exists() else 0 for suffix in ['', '-wal', '-shm']}
            report['probe_wal_checkpoint'] = list(conn.execute('PRAGMA wal_checkpoint(PASSIVE)').fetchone())
            report['files_after_probe_checkpoint'] = {suffix: (args.cell_dir / ('room.db'+suffix)).stat().st_size if (args.cell_dir / ('room.db'+suffix)).exists() else 0 for suffix in ['', '-wal', '-shm']}
        else:
            report['database_physical_bytes'] = scalar('SELECT pg_database_size(current_database())')
            report['relation_total_bytes'] = scalar("SELECT sum(pg_total_relation_size(oid)) FROM pg_class WHERE relnamespace='public'::regnamespace AND relkind='r'")
            report['table_heap_bytes'] = scalar("SELECT sum(pg_table_size(oid)) FROM pg_class WHERE relnamespace='public'::regnamespace AND relkind='r'")
            report['index_bytes'] = scalar("SELECT sum(pg_indexes_size(oid)) FROM pg_class WHERE relnamespace='public'::regnamespace AND relkind='r'")
            report['server_version'] = psql('SHOW server_version;')
            report['client_version'] = captured(['psql','--version'])
            report['postgres_settings'] = {key:psql('SHOW '+key+';') for key in ['synchronous_commit','checkpoint_timeout','max_wal_size']}
            report['wal_lsn'] = psql('SELECT pg_current_wal_lsn();')
            report['wal_attribution'] = 'Cluster-wide LSN; parallel database workloads prevent per-cell write attribution.'
        print(json.dumps(report))
        return 0
    else:
        raise RuntimeError('unknown probe')
    print(json.dumps({'fault_setup':args.probe, 'canonical_history_modified':False}))
    return 0


def metadata(binary):
    diff = subprocess.run(['git','diff','HEAD','--binary'], cwd=ROOT, capture_output=True, check=True).stdout
    host = {}
    if sys.platform == 'darwin':
        host = {'cpu_model':captured(['sysctl','-n','machdep.cpu.brand_string']), 'physical_memory_bytes':int(captured(['sysctl','-n','hw.memsize']))}
    return {'git_revision':captured(['git','rev-parse','HEAD']), 'tracked_dirty_diff_sha256':hashlib.sha256(diff).hexdigest(), 'source_sha256':digest(RUST_SOURCE), 'driver_sha256':digest(Path(__file__)), 'binary_sha256':digest(binary), 'cargo_lock_sha256':digest(ROOT/'Cargo.lock'), 'os':platform.platform(), 'logical_cores':os.cpu_count(), **host, 'scratch_free_bytes':shutil.disk_usage(ROOT).free, 'rust':captured(['rustc','--version']), 'cargo':captured(['cargo','--version']), 'rss_interval_seconds':0.05, 'quantiles':'At most 4096 deterministic evenly spaced latency samples; exact quantiles within sample', 'engine_daemon_rss':'not_measured; Rust process RSS excludes PostgreSQL server and Docker VM', 'native_backup_transfer':'root-owned separate qualification', 'dsn_values':'excluded'}


def prepare_pg(binary, directory, name):
    admin = os.environ['WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN']
    runtime = os.environ['WORLDSTREAM_POSTGRES_TEST_DSN']
    role = dsn_fields(runtime)['user']
    if not role.replace('_','').isalnum():
        raise RuntimeError('unexpected runtime role')
    psql(f'CREATE DATABASE {name};', admin)
    env = os.environ.copy()
    env['WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN'] = replace_database(admin,name)
    env['WORLDSTREAM_POSTGRES_TEST_DSN'] = replace_database(runtime,name)
    for filename, value in [('admin.dsn',env['WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN']),('runtime.dsn',env['WORLDSTREAM_POSTGRES_TEST_DSN'])]:
        path = directory/filename
        path.write_text(value+'\n')
        path.chmod(0o600)
    captured([str(binary),'--backend','postgres','--cell-dir',str(directory),'--output',str(directory/'setup.json'),'--setup-postgres'],env=env)
    sql=f"""REVOKE CREATE ON SCHEMA public FROM PUBLIC, {role}; GRANT USAGE ON SCHEMA public TO {role}; GRANT SELECT ON ALL TABLES IN SCHEMA public TO {role};
DO $g$ DECLARE r record; BEGIN FOR r IN SELECT tablename FROM pg_tables WHERE schemaname='public' AND tablename<>'worldstream_schema_migrations' AND tablename NOT LIKE 'worldstream_transfer_%' LOOP EXECUTE format('GRANT INSERT, UPDATE ON TABLE public.%I TO %I',r.tablename,'{role}'); IF r.tablename<>'worldstream_frames' THEN EXECUTE format('GRANT DELETE ON TABLE public.%I TO %I',r.tablename,'{role}'); END IF; END LOOP; END $g$;
GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA public TO {role};"""
    psql(sql,env['WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN'])
    return env


def run_cell(args, backend, fmt, count, size, directory, ordinal):
    directory.mkdir(mode=0o700)
    env = os.environ.copy()
    database = None
    source_directory=args.existing_cell_dir.resolve() if args.existing_cell_dir else directory
    if backend=='postgres' and args.existing_cell_dir:
        env['WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN']=(source_directory/'admin.dsn').read_text().strip()
        env['WORLDSTREAM_POSTGRES_TEST_DSN']=(source_directory/'runtime.dsn').read_text().strip()
        database=dsn_fields(env['WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN'])['dbname']
    elif backend=='postgres':
        database='ws_scaling_'+str(os.getpid())+'_'+str(ordinal)
        env=prepare_pg(args.binary,directory,database)
    argv=[str(args.binary),'--backend',backend,'--format',fmt,'--transitions',str(count),'--state-bytes',str(size),'--cell-dir',str(source_directory),'--output',str(directory/'cell.json')]
    if args.existing_cell_dir:
        argv.append('--existing-recovery')
    time_tool=Path('/usr/bin/time')
    if time_tool.exists():
        argv=[str(time_tool),'-l' if sys.platform=='darwin' else '-v',*argv]
    baseline=None;peak=0;samples=0;began=time.monotonic();omitted=None;last_disk_probe=began
    with (directory/'stdout.log').open('w') as stdout, (directory/'time.log').open('w') as stderr:
        child=subprocess.Popen(argv,cwd=ROOT,env=env,stdout=stdout,stderr=stderr,start_new_session=True)
        try:
            while child.poll() is None:
                # Sample Rust process only; engine and driver are separate.
                rows=captured(['ps','-axo','pid=,ppid=,rss=']).splitlines()
                rss=[]
                for row in rows:
                    fields=row.split()
                    if len(fields)==3:
                        pid,ppid,value=map(int,fields)
                        if (time_tool.exists() and ppid==child.pid) or (not time_tool.exists() and pid==child.pid):
                            rss.append(value*1024)
                if rss:
                    current=sum(rss);baseline=current if baseline is None else baseline;peak=max(peak,current);samples+=1
                physical=sum(p.stat().st_size for p in source_directory.iterdir() if p.is_file() and p.name.startswith('room.db'))
                if backend == 'postgres' and time.monotonic()-last_disk_probe >= 10:
                    physical=int(psql('SELECT pg_database_size(current_database());',env['WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN']))
                    last_disk_probe=time.monotonic()
                if physical>args.disk_budget_bytes:
                    omitted='actual backend physical storage exceeded explicit disk budget'
                    os.killpg(child.pid,signal.SIGTERM)
                    break
                time.sleep(0.05)
            status=child.wait()
        except BaseException:
            os.killpg(child.pid,signal.SIGTERM)
            child.wait()
            raise
    maximum=None
    for line in (directory/'time.log').read_text().splitlines():
        if sys.platform=='darwin' and 'maximum resident set size' in line:
            maximum=int(line.split()[0])
        elif 'Maximum resident set size (kbytes):' in line:
            maximum=int(line.rsplit(':',1)[1])*1024
    result={'backend':backend,'format':fmt,'transitions':count,'state_bytes':size,'status':'omitted' if omitted else ('passed' if status==0 else 'failed'),'exit_code':status,'elapsed_seconds':time.monotonic()-began,'rss_sampled_baseline_bytes':baseline,'rss_sampled_peak_bytes':peak if samples else None,'rss_samples':samples,'rust_time_max_rss_bytes':maximum,'cell_directory':directory.name,'postgres_disposable_database':database,'database_retained_for_native_qualification':database is not None,'disk_guard_bytes':args.disk_budget_bytes,'disk_guard_interval_seconds':10 if backend=='postgres' else 0.05,'command_contract':'Rust child --backend --format --transitions --state-bytes --cell-dir --output; no DSN argument'}
    if omitted:
        result['omission_reason']=omitted
    if status==0:
        result['measurement']=json.loads((directory/'cell.json').read_text())
    if args.existing_cell_dir:
        result['existing_completed_source']=str(source_directory)
        result['mode']='existing-recovery; no mutation-throughput rerun'
    return result


def numbers(value):
    result=[int(item) for item in value.split(',')]
    if any(item<0 for item in result):
        raise argparse.ArgumentTypeError('nonnegative integer list required')
    return result


def audit_report(path):
    """Audit completed cells only; never attach a second SQLite engine to a live cell."""
    report=json.loads(path.read_text())
    cells=[]
    for cell in report['cells']:
        if cell['status']!='passed':
            continue
        directory=path.parent/cell['cell_directory']
        env=os.environ.copy()
        dsn=None
        if cell['backend']=='postgres':
            dsn=(directory/'admin.dsn').read_text().strip()
            env['WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN']=dsn
        child=subprocess.Popen(['/usr/bin/python3',str(Path(__file__).resolve()),'--probe','records','--backend',cell['backend'],'--cell-dir',str(directory)],cwd=ROOT,env=env,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True)
        semantic=hashlib.sha256(); count=0
        for line in child.stdout:
            record=json.loads(bytes.fromhex(line.strip()))
            # Both selected codecs retain these exact state/effect commitments.
            logical={key:record[key] for key in ['room_seq','resulting_core_state_hash','resulting_activity_state_hash','resulting_authoritative_state_hash','ordered_domain_events','ordered_attention_signals','ordered_timer_changes']}
            semantic.update(json.dumps(logical,sort_keys=True,separators=(',',':')).encode())
            count+=1
        if child.wait()!=0 or count!=cell['transitions']:
            raise RuntimeError('completed semantic stream audit failed')
        measured=cell['measurement']; inventory={}
        connection=sqlite3.connect(directory/'room.db',timeout=60) if dsn is None else None
        for name in ['ordinary_recovery','nonzero_tail_recovery','invalid_checkpoint_fallback','absent_checkpoint_full_replay']:
            receipt=measured[name]
            lower=receipt['checkpoint_seq'] if receipt['path']=='Checkpoint' else 0
            table='transitions' if connection else 'worldstream_transitions'
            length='length' if connection else 'octet_length'
            query=f"SELECT count(*),coalesce(sum({length}(transition_bytes)),0) FROM {table} WHERE room_id='{ROOM}' AND room_seq>{lower}"
            if connection:
                records,total=connection.execute(query).fetchone()
            else:
                records,total=json.loads(psql('SELECT row_to_json(t) FROM ('+query+') t;',dsn)).values()
            delivered=receipt['tail_records_delivered'] if receipt['path']=='Checkpoint' else receipt['prefix_records_delivered']
            if records!=delivered:
                raise RuntimeError('completed recovery record inventory mismatch')
            inventory[name]={'actual_stored_records':records,'actual_stored_bytes':total,'measurement_phase':'post-run SQL sum over exact receipt-selected suffix; elapsed recovery excludes this audit'}
        if connection:
            connection.close()
        cells.append({'backend':cell['backend'],'format':cell['format'],'transitions':cell['transitions'],'state_bytes':cell['state_bytes'],'per_transition_logical_sha256':semantic.hexdigest(),'actual_records':count,'recovery_byte_inventory':inventory})
    output=path.parent/'semantic-recovery-audit.json'
    output.write_text(json.dumps({'schema':'worldstream/backend-post-run-audit/v1','source_report':path.name,'cells':cells},indent=2)+'\n')
    print('audited '+str(len(cells))+' completed cells',flush=True)
    return 0


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--probe',choices=['records','metrics','checkpoint-due','fence','tail','invalid','absent'])
    parser.add_argument('--backend',choices=['sqlite','postgres'])
    parser.add_argument('--cell-dir',type=Path)
    parser.add_argument('--backends',default='sqlite,postgres')
    parser.add_argument('--formats',default='v1,v2')
    parser.add_argument('--transitions',type=numbers,default=[100,10000,100000])
    parser.add_argument('--state-bytes',type=numbers,default=[0])
    parser.add_argument('--disk-budget-bytes',type=int,default=536870912)
    parser.add_argument('--output-dir',type=Path)
    parser.add_argument('--binary',type=Path,default=ROOT/'target/release/examples/measure_backend_stream_scaling')
    parser.add_argument('--audit-report',type=Path)
    parser.add_argument('--existing-cell-dir',type=Path)
    args=parser.parse_args()
    if args.audit_report:
        return audit_report(args.audit_report.resolve())
    if args.probe:
        return probe(args)
    if not args.output_dir or not args.binary.is_file():
        parser.error('provide a fresh output directory and build the Rust example first')
    if args.existing_cell_dir and any(len(items)!=1 for items in [args.backends.split(','),args.formats.split(','),args.transitions,args.state_bytes]):
        parser.error('existing recovery requires one backend, format, transition count and state size')
    args.output_dir=args.output_dir.resolve();args.binary=args.binary.resolve();args.output_dir.mkdir(parents=True,exist_ok=False)
    report={'schema':'worldstream/backend-stream-driver/v1','metadata':metadata(args.binary),'cells':[]}
    for backend in args.backends.split(','):
        for count in args.transitions:
            for size in args.state_bytes:
                for fmt in args.formats.split(','):
                    label=f'{backend}-{fmt}-{count}-{size}'
                    print('starting '+label,flush=True)
                    cell=run_cell(args,backend,fmt,count,size,args.output_dir/label,len(report['cells']))
                    report['cells'].append(cell)
                    (args.output_dir/'report.json').write_text(json.dumps(report,indent=2)+'\n')
                    print(label+' '+cell['status']+' seconds='+str(round(cell['elapsed_seconds'],2)),flush=True)
                    if cell['status']=='failed':
                        return 1
    pairs={}
    for cell in report['cells']:
        if cell['status']=='passed':
            m=cell['measurement'];key=(cell['backend'],cell['transitions'],cell['state_bytes']);pairs.setdefault(key,[]).append(m)
    for pair in pairs.values():
        if len(pair)==2:
            for field in ['final_core_hash','final_activity_hash','effects_blake3']:
                if pair[0][field]!=pair[1][field]:
                    raise RuntimeError('paired semantic parity failed: '+field)
    report['paired_semantic_parity']='passed for completed pairs'
    (args.output_dir/'report.json').write_text(json.dumps(report,indent=2)+'\n')
    return 0


if __name__=='__main__':
    try:
        sys.exit(main())
    except (RuntimeError,KeyError,ValueError) as error:
        print('backend measurement driver failed: '+str(error),file=sys.stderr)
        sys.exit(1)
