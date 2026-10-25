# The cloud API on AWS

[`cloud-api.yaml`](cloud-api.yaml) is a CloudFormation template that runs the
cloud API of Open Pointcloud Studio (`open-pointcloud-studio-api`, see
[`docs/cloud-api.md`](../../docs/cloud-api.md)) on Amazon ECS with AWS Fargate
behind an Application Load Balancer. [`deploy.sh`](deploy.sh) deploys it.

Status: **prepared, not deployed.** The Container workflow lints the template
on every change (`cfn-lint`, pinned with hashes in
[`cfn-lint-requirements.txt`](cfn-lint-requirements.txt)) and tests
`deploy.sh` with a stand-in for the AWS command line; no workflow has AWS
credentials or deploys. A deployment is made by the owner of the AWS account
with the steps below.

## Why ECS on Fargate

- **Size.** A call carries the point-cloud file in its body, up to 64 MiB. A
  synchronous AWS Lambda invocation takes at most 6 MB and an API Gateway REST
  API 10 MB; the Application Load Balancer sets no limit on the body.
- **Time.** A call may compute for up to 300 s (`OPENAEC_API_TIJDSLIMIET_S`).
  The idle timeout of the load balancer is set to 330 s.
- **Disk.** Every call works in a folder of its own, which can take
  gigabytes; the ephemeral storage of a Fargate task goes up to 200 GiB.
- **One roll-out.** The shared HTTP contract of the OpenAEC SuperCloud names
  containers on ECS with Fargate for all its services.

## What the template creates

| Resource | Settings |
| --- | --- |
| Network | a VPC `10.80.0.0/16` with an internet gateway, two public subnets in two availability zones and their route table; no NAT gateway |
| `LoadBalancerSecurityGroup` | 443 and 80 from `AllowedCidr` |
| `ServiceSecurityGroup` | 8080 from the load balancer only |
| `LoadBalancer` | internet-facing; idle timeout 330 s; invalid header fields dropped |
| `HttpsListener` | 443 with `CertificateArn` and the policy `ELBSecurityPolicy-TLS13-1-2-2021-06` (TLS 1.2 and 1.3). `/v1/*` and `/mcp` go to the service; any other path gets a fixed JSON 404, `{"error_code":"ROUTE_ONBEKEND",...}`, as the service answers itself |
| `HttpListener` | 80, redirected to 443 |
| `TargetGroup` | the tasks by IP address on port 8080; health `GET /v1/health` with 200, every 30 s, timeout 10 s, unhealthy after 5 failures, healthy after 2; deregistration delay 330 s, so that a running call can end when its task is replaced |
| `LogGroup` | `/openaec/open-pointcloud-studio-api`, kept 30 days |
| `ExecutionRole` | for ECS only: write to the log group, read the key secret (and the registry secret when given). No managed policy; the task has no role, because the service calls no AWS service |
| `Cluster` | Fargate |
| `TaskDefinition` | Linux on x86-64, 4 vCPU and 8 GiB by default, 50 GiB of ephemeral storage; the container below |
| `Service` | `DesiredCount` tasks (1) in the public subnets with a public address, so that they pull the image from ghcr.io without a NAT gateway; only the load balancer reaches them. The deployment circuit breaker rolls a failed deployment back; health checks begin after 30 s |

The container of the task:

- the image by its digest, as user `10001:10001`, with a read-only root file
  system and port 8080;
- the volume `work` at `/work`, where the work folders of the calls are made;
- the environment of the image (`OPENAEC_MCP_HTTP=0.0.0.0:8080`,
  `OPENAEC_API_LOGVORM=json`, `OPENAEC_API_WERKMAP=/work`, `TMPDIR=/work/tmp`),
  `OPENAEC_API_GEHEUGEN_MIB` set to the memory of the task, the limits from the
  parameters and, when given, `RAYON_NUM_THREADS`;
- `OPENAEC_API_SLEUTELS` from the secret `ApiKeysSecretArn`. The template has
  no setting that starts the service without keys, and the service itself
  refuses open access when it sees the variables of ECS;
- the container health check `open-pointcloud-studio-api --health-check`
  (every 30 s, timeout 10 s, 5 retries, 30 s to start), 120 s to stop after
  SIGTERM, an init process, and the log to the log group.

## Parameters

| Parameter | Default | Meaning |
| --- | --- | --- |
| `ImageUri` | | the image by its digest: `ghcr.io/openaec-foundation/open-pointcloud-studio-api@sha256:...` from the release notes |
| `HostName` | | the host name of the service, which the certificate covers |
| `CertificateArn` | | the certificate of AWS Certificate Manager for `HostName`, in the region of the stack |
| `ApiKeysSecretArn` | | the secret with the SHA-256 of each key, comma-separated |
| `RegistryCredentialsSecretArn` | empty | a secret with a username and password for the registry, while the image is not public |
| `AllowedCidr` | `0.0.0.0/0` | who may reach the load balancer |
| `DesiredCount` | 1 | tasks; 0 stops the service and keeps the rest |
| `Cpu`, `Memory` | 4096, 8192 | CPU units and MiB of a task: 2048 or 4096 CPU with 8192 or 16384 MiB |
| `EphemeralStorageGiB` | 50 | the disk of a task, 21 to 200 GiB |
| `MaxBodyMib`, `MaxOutputMib` | 64, 256 | `OPENAEC_API_MAX_LICHAAM_MIB`, `OPENAEC_API_MAX_UITKOMST_MIB` |
| `ConcurrentCalls`, `WaitingRoom` | 2, 4 | `OPENAEC_API_GELIJKTIJDIG`, `OPENAEC_API_WACHTRIJ` |
| `TimeLimitSeconds` | 300 | `OPENAEC_API_TIJDSLIMIET_S`, at most 300, below the idle timeout of the load balancer |
| `RayonThreads` | empty | `RAYON_NUM_THREADS`; empty is one thread per vCPU, which the calls of a task share |

The service checks at start that its worst case fits the memory
(`docs/cloud-api.md`, Limits): with the defaults about 3.5 GiB of 8 GiB. With
larger limits or more calls at once, give the task more memory, or it does not
start and the circuit breaker rolls back.

## What the owner provides

1. An AWS account and a region, and the AWS command line signed in as someone
   who may create the resources above, an IAM role among them.
2. A certificate for the host name in AWS Certificate Manager, in that region.
3. The keys: a long random key for each client, and their hashes in a secret:

   ```bash
   printf %s "<key>" | sha256sum
   aws secretsmanager create-secret --name openaec/open-pointcloud-studio-api/sleutels \
     --secret-string "<hash1>,<hash2>"
   ```

   A secret encrypted with a key of AWS KMS of its own also needs
   `kms:Decrypt` on that key for the execution role; the default key of the
   account needs nothing more.
4. The image: the release that publishes it, and the package
   `open-pointcloud-studio-api` made public in the organisation's packages
   on GitHub, or `RegistryCredentialsSecretArn` with a token that may read it.
5. The go for the deployment, which costs money while it runs (below).

## Deploy and test

```bash
export AWS_REGION=eu-central-1
image=ghcr.io/openaec-foundation/open-pointcloud-studio-api@sha256:...   # from the release notes
bash deploy/aws/deploy.sh open-pointcloud-studio-api "$image" "$CERTIFICATE_ARN" "$SECRET_ARN" pointcloud-api.example.org
```

This creates a change set and shows it; nothing runs yet. After looking at it:

```bash
OPENAEC_SMOKE_KEY=<one of the keys> bash deploy/aws/deploy.sh open-pointcloud-studio-api \
  "$image" "$CERTIFICATE_ARN" "$SECRET_ARN" pointcloud-api.example.org --execute
```

executes the change set, waits for the stack, and runs
[`../cloud-api/smoke.sh`](../cloud-api/smoke.sh) against the load balancer of
the stack under the host name, with its load step: two section drawings of
the largest test file at once, then health. Further parameters go after the
host name as `NAME=VALUE`, such as `DesiredCount=2`.

Then point the DNS record of the host name at the output
`LoadBalancerDnsName` (a CNAME, or an alias record in Route 53), and run the
smoke test once more by the name alone:

```bash
bash deploy/cloud-api/smoke.sh https://pointcloud-api.example.org "<key>" --version <version>
```

The log is in CloudWatch Logs under `/openaec/open-pointcloud-studio-api`, one
JSON object per line; requests name the first eight digits of the hash of
their key, never the key or a body.

To remove everything: `aws cloudformation delete-stack --stack-name open-pointcloud-studio-api`.
The log group goes with it; the secret and the certificate stay.

## Volume ownership

The image declares `VOLUME ["/work"]` with the folder owned by the user of the
service, so that the volume of the task takes that owner, and the service
checks at start that it can write in `/work` and `/work/tmp`; when it cannot, it
names the folder in its log, does not start, and the circuit breaker rolls
back. The first deployment records which it was in `docs/ecosystem.md`. If the
folder turns out to belong to root, the fallback is a second container in the
task that is not essential and runs first: `user: root`, the command
`chown 10001:10001 /work && mkdir -p /work/tmp`, the same mount point, and a
`DependsOn` of the service container on its `SUCCESS`.

## Cost

The default, with one task always on, in the prices of us-east-1 (check the
current prices of the region; other regions cost more):

| Part | A month |
| --- | --- |
| Fargate: 4 vCPU and 8 GiB, 730 hours | about USD 145 |
| Fargate: 30 GiB of ephemeral storage above the 20 GiB included | about USD 3 |
| Application Load Balancer, with light traffic | about USD 20 |
| Public IPv4 addresses: two of the load balancer and one of the task | about USD 11 |
| CloudWatch Logs and Secrets Manager | a few dollars |

About USD 170 to 180 a month in all. `DesiredCount=0` stops the tasks and
their cost; the load balancer and its addresses cost while the stack exists.

## Not in this template

Autoscaling, a web application firewall, access logs of the load balancer, the
DNS record, an arm64 image and sign-in with the OpenAEC account are not part of
it yet.
