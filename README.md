# Azure Email Communication Service  for Rust (azure-ecs-rs)

This is a Rust library for interacting with the Azure Email Communication Service (ECS) API.
[Azure Communication Service - Email - Rest API](https://learn.microsoft.com/en-us/rest/api/communication/email/send?tabs=HTTP)

Before running the examples, you need to set the following environment variables:

```aiignore
# For Common
SENDER="xxx
REPLY_EMAIL="xxxx"
REPLY_EMAIL_DISPLAY="xxxx"

# For Shared Key
CONNECTION_STR="xxxxx"

# For SMTP
SMTP_USER="xxxx"
SMTP_PASSWORD="xxxx"
SMTP_SERVER="smtp.azurecomm.net"

# For Service Principle
CLIENT_ID="xx"
CLIENT_SECRET="xxx"
TENANT_ID="xxx"
ASC_URL="https://xxxxx.asiapacific.communication.azure.com"

```
My example code is in the `examples` directory. You can run the examples with:
```sh
RUST_LOG=debug cargo run --example mail
```
