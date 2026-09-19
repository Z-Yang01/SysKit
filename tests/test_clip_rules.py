"""syskit/cliprules.py 单元测试：纯逻辑检测，无网络/无 Qt。"""
import os

from syskit import cliprules


def _ids(text):
    return [h["id"] for h in cliprules.detect(text)]


class TestJson:
    def test_object_formatted(self):
        hits = cliprules.detect('{"a":1,"b":"中文"}')
        fmt = [h for h in hits if h["id"] == "json_format"]
        assert len(fmt) == 1
        assert '"a": 1' in fmt[0]["arg"] and '"b": "中文"' in fmt[0]["arg"]
        assert fmt[0]["online"] is False

    def test_array(self):
        assert "json_format" in _ids("[1, 2]")

    def test_invalid_json_no_hit(self):
        assert "json_format" not in _ids('{"a": nope}')

    def test_non_json_prefix_no_hit(self):
        assert _ids("normal text") == []


class TestUrl:
    def test_plain_url(self):
        hits = cliprules.detect("https://example.com/a?b=1")
        ids = [h["id"] for h in hits]
        assert "url_qr" in ids and "url_open" in ids

    def test_url_extracted_from_sentence_and_trailing_punct(self):
        hits = cliprules.detect("看这个 https://example.com/x.")
        url_hits = [h for h in hits if h["id"] == "url_open"]
        assert url_hits[0]["arg"] == "https://example.com/x"

    def test_http_only(self):
        assert "url_open" in _ids("http://192.168.1.1/")


class TestColor:
    def test_hex6(self):
        hits = cliprules.detect("#FF00AA")
        assert hits[0]["id"] == "color_show" and hits[0]["arg"] == "#FF00AA"

    def test_hex3_lowercase(self):
        hits = cliprules.detect("#f0a")
        assert hits[0]["id"] == "color_show" and hits[0]["arg"] == "#F0A"

    def test_bad_hex_no_hit(self):
        assert "color_show" not in _ids("#12345")
        assert "color_show" not in _ids("#GGHHII")


class TestFilePath:
    def test_existing_file(self, tmp_path):
        f = tmp_path / "a.txt"
        f.write_text("x", encoding="utf-8")
        hits = cliprules.detect(str(f))
        ids = [h["id"] for h in hits]
        assert "file_reveal" in ids and "file_open" in ids
        reveal = [h for h in hits if h["id"] == "file_reveal"][0]
        assert reveal["arg"] == str(f)

    def test_nonexistent_path_no_hit(self):
        assert "file_reveal" not in _ids("C:\\not\\exist\\path.txt")

    def test_relative_text_no_hit(self):
        assert _ids("hello world") == []


class TestIpDomain:
    def test_ipv4(self):
        ids = _ids("1.2.3.4")
        assert "ip_ping" in ids and "ip_geo" in ids
        assert "domain_dns" not in ids

    def test_ipv6_ping_only(self):
        ids = _ids("::1")
        assert "ip_ping" in ids and "ip_geo" not in ids

    def test_invalid_ip_treated_as_text(self):
        assert _ids("999.1.1.1") == []          # TLD 是数字，域名也不匹配

    def test_domain(self):
        hits = cliprules.detect("Example.COM")
        by_id = {h["id"]: h for h in hits}
        assert "domain_dns" in by_id and "domain_ping" in by_id
        assert by_id["domain_dns"]["arg"] == "example.com"
        assert by_id["domain_dns"]["online"] is True

    def test_single_word_no_hit(self):
        assert _ids("localhost") == []          # 无点号，非合法域名

    def test_url_priority_over_domain(self):
        ids = _ids("https://example.com")
        assert "domain_dns" not in ids and "url_open" in ids


class TestEdge:
    def test_empty_and_none(self):
        assert cliprules.detect("") == []
        assert cliprules.detect(None) == []
        assert cliprules.detect("   ") == []

    def test_oversize_text_rejected(self):
        assert cliprules.detect("a" * 1_000_001) == []

    def test_combined_hits_json_then_url(self):
        hits = cliprules.detect('{"u": "https://x.com"}')
        ids = [h["id"] for h in hits]
        assert "json_format" in ids and "url_qr" in ids

    def test_hit_shape(self):
        for hit in cliprules.detect("1.2.3.4"):
            assert set(hit) == {"id", "label", "arg", "online"}


class TestUrlCjkAndParens:
    """中文语境 URL 边界：尾部粘连标点/全角字符曾一起被抓进 arg（二维码/打开
    链接拿到错误地址）；维基式括号 URL 的右括号又曾被无条件误剥。"""

    def test_cjk_punctuation_after_url_stripped(self):
        hits = cliprules.detect("详情见 https://example.com/x。谢谢")
        arg = [h for h in hits if h["id"] == "url_open"][0]["arg"]
        assert arg == "https://example.com/x"

    def test_fullwidth_parens_around_url(self):
        hits = cliprules.detect("（https://example.com/a）")
        arg = [h for h in hits if h["id"] == "url_open"][0]["arg"]
        assert arg == "https://example.com/a"

    def test_balanced_ascii_parens_preserved(self):
        url = "https://en.wikipedia.org/wiki/Python_(programming_language)"
        hits = cliprules.detect(f"见 {url} 说明")
        arg = [h for h in hits if h["id"] == "url_open"][0]["arg"]
        assert arg == url

    def test_unbalanced_trailing_paren_stripped(self):
        hits = cliprules.detect("(see https://example.com/a)")
        arg = [h for h in hits if h["id"] == "url_open"][0]["arg"]
        assert arg == "https://example.com/a"
