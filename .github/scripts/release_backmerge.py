#!/usr/bin/env python3
"""Open a CI-gated, release-SHA-pinned backmerge; never push integration branches."""

import json
import os
import re
import subprocess
import sys
import time
from urllib.parse import quote, urlencode


CHECKS = {
    "stable / default",
    "stable / no-default-features",
    "fmt and clippy",
    "ncm-api tests",
    "AUR packaging integration",
}


class BackmergeError(Exception):
    pass


def api(endpoint, payload=None, *, missing_ok=False):
    command = ["gh", "api", "--method", "GET" if payload is None else "POST", endpoint]
    if payload is not None:
        command += ["--input", "-"]
    result = subprocess.run(
        command, input=None if payload is None else json.dumps(payload),
        text=True, capture_output=True, check=False,
    )
    if result.returncode:
        if missing_ok and "(HTTP 404)" in result.stderr:
            return None
        raise BackmergeError(f"GitHub API {endpoint} failed: {result.stderr.strip()}")
    response = json.loads(result.stdout)
    if isinstance(response, dict) and response.get("errors"):
        raise BackmergeError(f"GitHub GraphQL error: {response['errors']}")
    return response


def graphql(query, variables):
    return api("graphql", {"query": query, "variables": variables})["data"]


def run():
    repository = os.environ.get("GH_REPO", "")
    tag = os.environ.get("RELEASE_TAG", "")
    sha = os.environ.get("RELEASE_SHA", "")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise BackmergeError("GH_REPO must identify owner/repository")
    if not re.fullmatch(r"v[0-9][A-Za-z0-9._-]*", tag) or ".." in tag or tag.endswith((".", ".lock")):
        raise BackmergeError("RELEASE_TAG must be a safe v-prefixed version tag")
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise BackmergeError("RELEASE_SHA must be the released commit SHA")
    prefix = f"repos/{repository}"
    owner, name = repository.split("/")
    branch = f"release-backmerge/{tag}"

    def ref(name, *, optional=False):
        return api(f"{prefix}/git/ref/{quote(name, safe='/')}", missing_ok=optional)

    def check_release():
        release = ref(f"tags/{tag}")["object"]
        while release["type"] == "tag":
            release = api(f"{prefix}/git/tags/{release['sha']}")["object"]
        if release["type"] != "commit" or release["sha"] != sha:
            raise BackmergeError("Tag no longer identifies the released commit; manual action required")

    def check_main():
        if ref("heads/main")["object"]["sha"] != sha:
            raise BackmergeError("main moved away from this release; manual backmerge required")

    check_release()
    comparison = api(f"{prefix}/compare/{sha}...develop")
    if comparison["status"] in ("identical", "ahead"):
        print(f"develop already contains {tag} ({sha}); no changes")
        return
    check_main()
    settings = api(prefix)
    if not settings["allow_auto_merge"] or not settings["allow_merge_commit"]:
        raise BackmergeError("Repository must allow auto-merge and merge commits")
    rules = api(f"{prefix}/rules/branches/develop")
    required = {
        check["context"]
        for rule in rules if rule["type"] == "required_status_checks"
        for check in rule["parameters"]["required_status_checks"]
        if check.get("integration_id") == 15368
    }
    if not CHECKS.issubset(required) or not any(rule["type"] == "pull_request" for rule in rules):
        raise BackmergeError("develop must require PRs and all five GitHub Actions CI checks")
    if any(rule["type"] == "merge_queue" for rule in rules):
        raise BackmergeError("Merge queues cannot guarantee the required MERGE method")

    existing = ref(f"heads/{branch}", optional=True)
    if existing is None:
        api(f"{prefix}/git/refs", {"ref": f"refs/heads/{branch}", "sha": sha})
    elif existing["object"]["sha"] != sha:
        raise BackmergeError("Existing backmerge branch has changed; refusing to rewrite it")
    query = urlencode({"state": "all", "head": f"{owner}:{branch}", "base": "develop", "per_page": 100})
    pulls = api(f"{prefix}/pulls?{query}")
    opened = [pull for pull in pulls if pull["state"] == "open"]
    if len(opened) > 1:
        raise BackmergeError("Multiple backmerge PRs exist; manual action required")
    if opened:
        number = opened[0]["number"]
    elif pulls:
        raise BackmergeError("Backmerge PR was closed but release is absent from develop; manual action required")
    else:
        pull = api(f"{prefix}/pulls", {
            "title": f"chore: backmerge {tag} into develop",
            "head": branch, "base": "develop",
            "body": (
                f"Release `{tag}` completed build, release tests, publication and AUR synchronization.\n\n"
                f"Pinned released main commit: `{sha}`. Required CI gates a merge commit into develop. "
                "Do not squash, rebase, update this branch or bypass checks. "
                "Conflicts require maintainer resolution; main remains manually maintained."
            ),
        })
        number = pull["number"]
    print(f"Backmerge PR: https://github.com/{repository}/pull/{number}")
    query = """query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){
      pullRequest(number:$number){id state isDraft headRefName headRefOid baseRefName
        headRepository{nameWithOwner} baseRepository{nameWithOwner} mergeable mergeStateStatus
        autoMergeRequest{mergeMethod}}}}"""
    variables = {"owner": owner, "name": name, "number": number}
    for attempt in range(6):
        pull = graphql(query, variables)["repository"]["pullRequest"]
        if (
            pull["state"] != "OPEN" or pull["isDraft"]
            or pull["headRefName"] != branch or pull["headRefOid"] != sha
            or pull["baseRefName"] != "develop"
            or (pull["headRepository"] or {}).get("nameWithOwner", "").lower() != repository.lower()
            or pull["baseRepository"]["nameWithOwner"].lower() != repository.lower()
        ):
            raise BackmergeError("PR target or pinned head changed; manual action required")
        if pull["mergeable"] != "UNKNOWN":
            break
        if attempt < 5:
            time.sleep(2)
    if pull["mergeable"] != "MERGEABLE":
        raise BackmergeError(f"PR mergeability is {pull['mergeable']}; PR retained for manual action")
    check_release()
    check_main()
    if ref(f"heads/{branch}")["object"]["sha"] != sha:
        raise BackmergeError("Backmerge branch changed before enabling auto-merge")
    if pull["autoMergeRequest"]:
        if pull["autoMergeRequest"]["mergeMethod"] != "MERGE":
            raise BackmergeError("Existing auto-merge uses the wrong method; manual action required")
        print("Merge-commit auto-merge already enabled; no changes")
        return
    if pull["mergeStateStatus"] == "CLEAN":
        # The dedicated App has no bypass: GitHub still enforces all branch rules.
        mutation = """mutation($input:MergePullRequestInput!){
          mergePullRequest(input:$input){pullRequest{state}}}"""
        result = graphql(mutation, {"input": {
            "pullRequestId": pull["id"], "expectedHeadOid": sha, "mergeMethod": "MERGE",
        }})
        if result["mergePullRequest"]["pullRequest"]["state"] != "MERGED":
            raise BackmergeError("GitHub did not confirm the rule-gated merge")
        print("Required checks already passed; merge commit completed under branch rules")
        return
    mutation = """mutation($input:EnablePullRequestAutoMergeInput!){
      enablePullRequestAutoMerge(input:$input){pullRequest{autoMergeRequest{mergeMethod}}}}"""
    result = graphql(mutation, {"input": {
        "pullRequestId": pull["id"], "expectedHeadOid": sha, "mergeMethod": "MERGE",
    }})
    request = result["enablePullRequestAutoMerge"]["pullRequest"]["autoMergeRequest"]
    if not request or request["mergeMethod"] != "MERGE":
        raise BackmergeError("GitHub did not confirm merge-commit auto-merge")
    print("Merge-commit auto-merge enabled; GitHub waits for required CI")


if __name__ == "__main__":
    try:
        run()
    except (BackmergeError, KeyError, ValueError, OSError) as error:
        print(f"::error::{error}", file=sys.stderr)
        sys.exit(1)
