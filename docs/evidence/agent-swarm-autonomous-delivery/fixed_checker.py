
import json, runpy
module = runpy.run_path("csv_tool.py")
parse, fmt = module["parse_records"], module["format_records"]
checks = []
def equal(name, actual, expected):
    checks.append((name, actual == expected))
def rejects(name, function, argument):
    try:
        function(argument)
    except ValueError:
        checks.append((name, True))
    else:
        checks.append((name, False))
equal("empty_records", parse("name,note\n"), [])
equal("plain", parse("name,note\nAda,hello\n"), [{"name":"Ada","note":"hello"}])
equal("quoted_comma", parse('name,note\nAda,"a,b"\n'), [{"name":"Ada","note":"a,b"}])
equal("escaped_quote", parse('name,note\nAda,"say ""hi"""\n'), [{"name":"Ada","note":'say "hi"'}])
equal("embedded_newline", parse('name,note\nAda,"first\nsecond"\n'), [{"name":"Ada","note":"first\nsecond"}])
equal("crlf", parse("name,note\r\nAda,hello\r\n"), [{"name":"Ada","note":"hello"}])
equal("unicode", parse("name,note\nZoë,東京\n"), [{"name":"Zoë","note":"東京"}])
equal("empty_fields", parse("name,note\n,\n"), [{"name":"","note":""}])
rejects("missing_header", parse, "")
rejects("wrong_header", parse, "note,name\nhello,Ada\n")
rejects("missing_field", parse, "name,note\nAda\n")
rejects("extra_field", parse, "name,note\nAda,hello,extra\n")
rejects("unclosed_quote", parse, 'name,note\nAda,"oops')
equal("empty_format", fmt([]), "name,note\n")
rows = [{"name":"Zoë, Ada", "note":'first\n"東京"'}, {"name":"", "note":""}]
try:
    equal("round_trip", parse(fmt(rows)), rows)
except (ValueError, TypeError):
    checks.append(("round_trip", False))
rejects("invalid_format_fields", fmt, [{"name":"Ada"}])

import subprocess, sys
to_json = module["to_json"]
equal("json_strings_preserved", json.loads(to_json("name,note\n001,true\n")), [{"name":"001","note":"true"}])
equal("json_escaping", json.loads(to_json('name,note\nZoë,"first\n""東京"""\n')), [{"name":"Zoë","note":'first\n"東京"'}])
equal("json_empty", json.loads(to_json("name,note\n")), [])
rejects("format_non_string", fmt, [{"name":"Ada","note":5}])
cli = subprocess.run([sys.executable,"-B","csv_tool.py"],input="name,note\nAda,hi\n",text=True,capture_output=True,timeout=5)
equal("cli_valid", (cli.returncode,json.loads(cli.stdout),cli.stderr), (0,[{"name":"Ada","note":"hi"}],""))
cli = subprocess.run([sys.executable,"-B","csv_tool.py"],input="wrong,header\n",text=True,capture_output=True,timeout=5)
equal("cli_invalid", (cli.returncode,cli.stdout,bool(cli.stderr.strip()),"Traceback" in cli.stderr), (2,"",True,False))
rejects("quote_in_unquoted_field", parse, 'name,note\nA,b"c\n')
rejects("junk_after_quoted_field", parse, 'name,note\nA,"b"x\n')
rejects("format_extra_keys", fmt, [{"name":"A","note":"B","extra":"C"}])
cr_rows = [{"name":"A\rB","note":"C\rD"}]
try:
    equal("carriage_return_roundtrip", parse(fmt(cr_rows)), cr_rows)
except (ValueError, TypeError):
    checks.append(("carriage_return_roundtrip", False))

failed = [name for name, passed in checks if not passed]
print(json.dumps({"cases":len(checks), "passed":len(checks)-len(failed), "failed":failed}))
raise SystemExit(0 if len(checks) == 26 and not failed else 1)
