#!/usr/bin/env python3
"""Focused regression for the production-derived Chime INVITE transformer."""
import pathlib
import types
import unittest

import chime_caller


class ChimeFixtureTest(unittest.TestCase):
    def test_shape_and_wire_framing_survive_hermetic_rewrite(self):
        fixture = pathlib.Path(__file__).with_name("fixtures") / "chime-invite.txt"
        args = types.SimpleNamespace(
            src_ip="127.0.0.61",
            rtp_port=36200,
            exten="+19709601891",
        )
        invite, ruri = chime_caller.build_invite_from_capture(
            fixture,
            args,
            "fixture-call@example.invalid",
            "fixture-from-tag",
            "z9hG4bKfixture",
        )

        chime_caller.validate_wire_message(invite)
        self.assertEqual(
            ruri, "sip:+19709601891@voice.murphytek.com:45070;transport=UDP"
        )
        self.assertEqual(invite.count("\r\nRecord-Route:"), 2)
        self.assertEqual(invite.count("\r\nVia:"), 2)
        self.assertIn("alias=10.0.35.192~44933~2", invite)
        self.assertIn("Call-ID: fixture-call@example.invalid\r\n", invite)
        self.assertIn("c=IN IP4 127.0.0.61", invite)
        self.assertIn("m=audio 36200 RTP/AVP 0 101", invite)


if __name__ == "__main__":
    unittest.main()
