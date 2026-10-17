import importlib.util
import json
import os
from pathlib import Path
import sys
import types
import unittest
from unittest.mock import patch

os.environ["WAKE_TOKEN"] = "test-token"
os.environ["KCODE_GATEWAY_HOST"] = "100.64.0.10"
sys.modules.setdefault("boto3", types.ModuleType("boto3"))
sys.modules["boto3"].client = lambda *_args, **_kwargs: None

SPEC = importlib.util.spec_from_file_location(
    "wake_lambda", Path(__file__).with_name("wake-lambda.py")
)
wake = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(wake)


class FakeEc2:
    def __init__(self, state="running"):
        self.state = state
        self.started = False

    def describe_instances(self, **_kwargs):
        return {"Reservations": [{"Instances": [{"State": {"Name": self.state}}]}]}

    def start_instances(self, **_kwargs):
        self.started = True


class InvocationDoesNotExist(Exception):
    pass


class FakeSsm:
    class exceptions:
        InvocationDoesNotExist = InvocationDoesNotExist

    def describe_instance_information(self, **_kwargs):
        return {"InstanceInformationList": [{"PingStatus": "Online"}]}

    def send_command(self, **_kwargs):
        return {"Command": {"CommandId": "command-1"}}

    def get_command_invocation(self, **_kwargs):
        return {
            "Status": "Success",
            "StandardOutputContent": "",
            "StandardErrorContent": "Pairing code: \x1b[1;37m123 456\x1b[0m",
        }


class WakeLambdaTests(unittest.TestCase):
    def event(self, method, *, token=None, body=None, query=None):
        headers = {"Authorization": f"Bearer {token}"} if token else {}
        return {
            "requestContext": {"http": {"method": method}},
            "headers": headers,
            "queryStringParameters": query,
            "body": json.dumps(body) if body is not None else None,
        }

    def test_landing_page_never_embeds_token(self):
        result = wake.handler(self.event("GET"), None)
        self.assertEqual(result["statusCode"], 200)
        self.assertNotIn("test-token", result["body"])
        self.assertIn("Content-Security-Policy", result["headers"])

    def test_legacy_query_redirects_token_into_fragment(self):
        result = wake.handler(self.event("GET", query={"t": "test-token"}), None)
        self.assertEqual(result["statusCode"], 302)
        self.assertEqual(result["headers"]["Location"], "/#t=test-token")

    def test_post_requires_bearer_token(self):
        result = wake.handler(self.event("POST", body={"action": "status"}), None)
        self.assertEqual(result["statusCode"], 403)

    def test_status_uses_ec2_and_ssm_health(self):
        ec2, ssm = FakeEc2(), FakeSsm()
        with patch.object(wake.boto3, "client", side_effect=[ec2, ssm]):
            result = wake.handler(
                self.event("POST", token="test-token", body={"action": "status"}), None
            )
        self.assertEqual(result["statusCode"], 200)
        self.assertTrue(json.loads(result["body"])["healthy"])

    def test_pairing_parses_ansi_output_and_returns_tailnet_host(self):
        result = wake.fetch_pair_code(FakeSsm())
        self.assertEqual(result["code"], "123456")
        self.assertEqual(result["host"], "100.64.0.10")
        self.assertEqual(result["uri"], "jcode://pair?host=100.64.0.10&port=7643&code=123456")


if __name__ == "__main__":
    unittest.main()
