#!/usr/bin/env bash
# Tests of deploy.sh with a stand-in for the AWS command line and for the
# smoke test, so that they run anywhere and never reach AWS: without
# --execute the script only makes a change set, with --execute it executes
# that, waits and runs the smoke test against the stack's load balancer,
# and it refuses what it cannot do before the first call.
#
#   test-deploy.sh
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The script with its template beside it, and the stand-ins.
mkdir -p "$work/deploy/aws" "$work/deploy/cloud-api" "$work/bin"
cp "$here/deploy.sh" "$here/cloud-api.yaml" "$work/deploy/aws/"
cat > "$work/deploy/cloud-api/smoke.sh" <<'EOF'
#!/usr/bin/env bash
echo "smoke $* via ${SMOKE_CONNECT_TO:-}" >> "$AWS_LOG"
EOF
# Writes down each call. A change set is made unless AWS_UNCHANGED is set;
# the stack is new when AWS_NEW is set.
cat > "$work/bin/aws" <<'EOF'
#!/usr/bin/env bash
echo "$*" >> "$AWS_LOG"
case "$*" in
    *"cloudformation deploy "*)
        if [[ -n "${AWS_UNCHANGED:-}" ]]; then
            echo "No changes to deploy. Stack example is up to date"
        else
            echo "Waiting for changeset to be created.."
            echo "Changeset created successfully. Run the following command to review changes:"
            echo "aws cloudformation describe-change-set --change-set-name arn:aws:cloudformation:eu-central-1:123456789012:changeSet/awscli-cloudformation-package-deploy-1/abc"
        fi
        ;;
    *"StackStatus"*) [[ -n "${AWS_NEW:-}" ]] && echo REVIEW_IN_PROGRESS || echo UPDATE_COMPLETE ;;
    *"'ServiceUrl'"*) echo https://api.example.org ;;
    *"'LoadBalancerDnsName'"*) echo balancer-1.eu-central-1.elb.amazonaws.com ;;
    *"'LogGroupName'"*) echo /openaec/open-pointcloud-studio-api ;;
esac
EOF
chmod +x "$work/bin/aws" "$work/deploy/cloud-api/smoke.sh"

image=ghcr.io/openaec-foundation/open-pointcloud-studio-api@sha256:$(printf '0%.0s' $(seq 1 64))
certificate=arn:aws:acm:eu-central-1:123456789012:certificate/00000000-0000-0000-0000-000000000000
secret=arn:aws:secretsmanager:eu-central-1:123456789012:secret:keys-AbCdEf

failures=0
passed() { echo "ok    $*"; }
wrong() {
    echo "WRONG $*"
    failures=$((failures + 1))
}

# deploy [VARIABLE=VALUE...] -- ARGUMENTS... runs the copy; the calls are in $work/aws.log.
deploy() {
    local settings=()
    while [[ "$1" != -- ]]; do
        settings+=("$1")
        shift
    done
    shift
    : > "$work/aws.log"
    env PATH="$work/bin:$PATH" AWS_LOG="$work/aws.log" ${settings[@]+"${settings[@]}"} \
        bash "$work/deploy/aws/deploy.sh" "$@" > "$work/out.log" 2>&1
}

if deploy -- stack "$image" "$certificate" "$secret" api.example.org; then
    wrong "deploy.sh runs without AWS_REGION"
elif [[ -s "$work/aws.log" ]] || ! grep -qF AWS_REGION "$work/out.log"; then
    wrong "deploy.sh does not refuse to run without AWS_REGION before calling AWS"
else
    passed "deploy.sh refuses to run without AWS_REGION"
fi

if deploy AWS_REGION=eu-central-1 -- stack ghcr.io/example/api:latest "$certificate" "$secret" api.example.org \
    || [[ -s "$work/aws.log" ]]; then
    wrong "deploy.sh takes an image by its tag"
else
    passed "deploy.sh takes an image by its digest only"
fi

if deploy AWS_REGION=eu-central-1 -- stack "$image" "$certificate" "$secret" api.example.org --execute \
    || [[ -s "$work/aws.log" ]]; then
    wrong "deploy.sh executes without OPENAEC_SMOKE_KEY, or calls AWS first"
else
    passed "deploy.sh --execute refuses to run without a key for the smoke test"
fi

if deploy AWS_REGION=eu-central-1 -- stack "$image" "$certificate" "$secret" api.example.org DesiredCount=2 \
    && grep -qF -- "--region eu-central-1 cloudformation deploy --stack-name stack" "$work/aws.log" \
    && grep -qF -- "--capabilities CAPABILITY_IAM --no-execute-changeset" "$work/aws.log" \
    && grep -qF -- "ImageUri=$image CertificateArn=$certificate ApiKeysSecretArn=$secret HostName=api.example.org DesiredCount=2" "$work/aws.log" \
    && grep -qF "describe-change-set --change-set-name arn:aws:cloudformation:eu-central-1:123456789012:changeSet/" "$work/aws.log" \
    && ! grep -qE 'execute-change-set|smoke' "$work/aws.log"; then
    passed "without --execute deploy.sh makes and shows a change set and executes nothing"
else
    wrong "without --execute deploy.sh does not stop at the change set:"
    sed 's/^/        /' "$work/aws.log" "$work/out.log"
fi

if deploy AWS_REGION=eu-central-1 AWS_NEW=1 OPENAEC_SMOKE_KEY=secret-key -- \
        stack "$image" "$certificate" "$secret" api.example.org --execute \
    && grep -qF "execute-change-set --change-set-name arn:aws:cloudformation:" "$work/aws.log" \
    && grep -qF "wait stack-create-complete --stack-name stack" "$work/aws.log" \
    && grep -qxF "smoke https://api.example.org secret-key --load via api.example.org:443:balancer-1.eu-central-1.elb.amazonaws.com:443" "$work/aws.log"; then
    passed "deploy.sh --execute creates the stack and runs the smoke test through its load balancer"
else
    wrong "deploy.sh --execute does not create the stack and run the smoke test:"
    sed 's/^/        /' "$work/aws.log" "$work/out.log"
fi

if deploy AWS_REGION=eu-central-1 OPENAEC_SMOKE_KEY=secret-key -- \
        stack "$image" "$certificate" "$secret" api.example.org --execute \
    && grep -qF "wait stack-update-complete --stack-name stack" "$work/aws.log"; then
    passed "deploy.sh --execute waits for an update of an existing stack"
else
    wrong "deploy.sh --execute does not wait for the update of an existing stack:"
    sed 's/^/        /' "$work/aws.log" "$work/out.log"
fi

if deploy AWS_REGION=eu-central-1 AWS_UNCHANGED=1 OPENAEC_SMOKE_KEY=secret-key -- \
        stack "$image" "$certificate" "$secret" api.example.org --execute \
    && ! grep -qE 'execute-change-set|wait ' "$work/aws.log" \
    && grep -q '^smoke ' "$work/aws.log"; then
    passed "deploy.sh --execute on an unchanged stack only runs the smoke test"
else
    wrong "deploy.sh --execute on an unchanged stack does more than the smoke test:"
    sed 's/^/        /' "$work/aws.log" "$work/out.log"
fi

if [[ "$failures" -ne 0 ]]; then
    echo "test-deploy.sh: $failures of the tests failed" >&2
    exit 1
fi
echo "all tests of deploy.sh passed"
