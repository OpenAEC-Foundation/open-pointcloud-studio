#!/usr/bin/env bash
# Deploys the cloud API to AWS with cloud-api.yaml, in two steps: a change
# set to look at first, and only with --execute its execution, followed by
# the smoke test against the deployed service.
#
#   deploy.sh STACK IMAGE_URI CERTIFICATE_ARN SECRET_ARN HOST_NAME [NAME=VALUE...] [--execute]
#
#   STACK            the name of the stack, such as open-pointcloud-studio-api
#   IMAGE_URI        the image by its digest, as the release notes give it:
#                    ghcr.io/openaec-foundation/open-pointcloud-studio-api@sha256:...
#   CERTIFICATE_ARN  the certificate of AWS Certificate Manager for HOST_NAME,
#                    in the region of the stack
#   SECRET_ARN       the secret of AWS Secrets Manager that holds the SHA-256
#                    of each key, comma-separated
#   HOST_NAME        the host name of the service, which the certificate covers
#   NAME=VALUE       other parameters of the template, such as DesiredCount=2
#                    or AllowedCidr=203.0.113.0/24
#
# AWS_REGION names the region. The AWS command line has to be signed in as
# someone who may create what the template holds, an IAM role among them
# (CAPABILITY_IAM). With --execute, OPENAEC_SMOKE_KEY is a key whose SHA-256
# is in the secret: the smoke test (../cloud-api/smoke.sh, with its load
# step) runs against the load balancer of the stack under HOST_NAME, also
# before the DNS record of HOST_NAME points there.
#
# Without --execute the account gets a change set and nothing else; the
# script prints how to execute or delete it. No workflow calls this script.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
fail() {
    echo "deploy.sh: $*" >&2
    exit 1
}

execute=false
arguments=()
for argument in "$@"; do
    if [[ "$argument" == --execute ]]; then
        execute=true
    else
        arguments+=("$argument")
    fi
done
[[ ${#arguments[@]} -ge 5 ]] \
    || fail "usage: deploy.sh STACK IMAGE_URI CERTIFICATE_ARN SECRET_ARN HOST_NAME [NAME=VALUE...] [--execute]"
stack=${arguments[0]}
image=${arguments[1]}
certificate=${arguments[2]}
secret=${arguments[3]}
host=${arguments[4]}
extra=("${arguments[@]:5}")

[[ -n "${AWS_REGION:-}" ]] || fail "set AWS_REGION to the region of the stack"
[[ "$image" == *@sha256:* ]] || fail "IMAGE_URI has to name the image by its digest (...@sha256:...), not '$image'"
for pair in ${extra[@]+"${extra[@]}"}; do
    [[ "$pair" == [A-Za-z]*=* ]] || fail "'$pair' is not a parameter NAME=VALUE"
done
if $execute && [[ -z "${OPENAEC_SMOKE_KEY:-}" ]]; then
    fail "--execute runs the smoke test afterwards: set OPENAEC_SMOKE_KEY to a key whose SHA-256 is in the secret"
fi

aws_() { aws --region "$AWS_REGION" "$@"; }

echo "Creating a change set for stack $stack in $AWS_REGION"
output=$(aws_ cloudformation deploy \
    --stack-name "$stack" \
    --template-file "$here/cloud-api.yaml" \
    --capabilities CAPABILITY_IAM \
    --no-execute-changeset \
    --no-fail-on-empty-changeset \
    --parameter-overrides \
        "ImageUri=$image" "CertificateArn=$certificate" "ApiKeysSecretArn=$secret" "HostName=$host" \
        ${extra[@]+"${extra[@]}"})
echo "$output"
change_set=$(grep -o 'arn:aws[a-z-]*:cloudformation:[^ ]*:changeSet/[^ ]*' <<< "$output" | head -n 1 || true)

if [[ -n "$change_set" ]]; then
    aws_ cloudformation describe-change-set --change-set-name "$change_set" \
        --query 'Changes[].ResourceChange.{Action: Action, Resource: LogicalResourceId, Type: ResourceType, Replacement: Replacement}' \
        --output table
fi

if ! $execute; then
    if [[ -n "$change_set" ]]; then
        echo
        echo "Nothing is deployed yet. To deploy this change set and run the smoke test:"
        echo "  OPENAEC_SMOKE_KEY=... bash deploy/aws/deploy.sh $* --execute"
        echo "To drop it:"
        echo "  aws --region $AWS_REGION cloudformation delete-change-set --change-set-name $change_set"
    fi
    exit 0
fi

if [[ -n "$change_set" ]]; then
    status=$(aws_ cloudformation describe-stacks --stack-name "$stack" --query 'Stacks[0].StackStatus' --output text)
    echo "Executing the change set (stack status $status)"
    aws_ cloudformation execute-change-set --change-set-name "$change_set"
    if [[ "$status" == REVIEW_IN_PROGRESS ]]; then
        aws_ cloudformation wait stack-create-complete --stack-name "$stack"
    else
        aws_ cloudformation wait stack-update-complete --stack-name "$stack"
    fi
else
    echo "The stack is up to date; only the smoke test runs."
fi

output_of() {
    aws_ cloudformation describe-stacks --stack-name "$stack" \
        --query "Stacks[0].Outputs[?OutputKey=='$1'].OutputValue" --output text
}
url=$(output_of ServiceUrl)
balancer=$(output_of LoadBalancerDnsName)
echo "Service $url, load balancer $balancer, log group $(output_of LogGroupName)"

SMOKE_CONNECT_TO="$host:443:$balancer:443" \
    bash "$here/../cloud-api/smoke.sh" "$url" "$OPENAEC_SMOKE_KEY" --load
