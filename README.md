# Azure Email Communication Service  for Rust (azure-ecs-rs)

Azure Email Communication Service is part of the Azure Communication Services. It provides a REST API to send emails.
For more information, see the [Azure Communication Services documentation](https://learn.microsoft.com/en-us/azure/communication-services/).

This crate provides a Rust client library for the Azure Email Communication Service. It supports the following features:
[Azure Communication Service - Email - Rest API](https://learn.microsoft.com/en-us/rest/api/communication/email/send?tabs=HTTP)


Core Features
- Send Email (sync and async)
- Get Email Status

Support Authentication:
- Shared Key
- Service Principle
- Managed Identity


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

# Host name
ASC_URL="https://xxxxx.asiapacific.communication.azure.com"

```
My example code is in the `examples` directory. You can run the examples with:
```sh
# For simple email
RUST_LOG=debug cargo run --example mail

# For simple email with async
RUST_LOG=debug cargo run --example mail_async

# For email with attachment
RUST_LOG=debug cargo run --example mail_attach
```
How to use the library:
- Shared Key

- Get from Azure Portal
  - CONNECTION_STR
    ![Alt text](https://github.com/preedep/rust_azure_email_communication/blob/develop/images/image2.png "Connection String")
  - SENDER
    ![Alt text](https://github.com/preedep/rust_azure_email_communication/blob/develop/images/image1.png "Sender")


```rust
 let connection_str = get_env_var("CONNECTION_STR");
 let acs_client_builder = ACSClientBuilder::new().connection_string(connection_str.as_str())
```
- Service Principle
```rust
 let host_name = get_env_var("ASC_URL");
 let tenant_id = get_env_var("TENANT_ID");
 let client_id = get_env_var("CLIENT_ID");
 let client_secret = get_env_var("CLIENT_SECRET");
 
 let acs_client_builder = ACSClientBuilder::new()
                .host(host_name.as_str())
                .service_principal(
                    tenant_id.as_str(),
                    client_id.as_str(),
                    client_secret.as_str(),
                )
```
- Managed Identity
```rust
 let host_name = get_env_var("ASC_URL");
 let acs_client_builder =  ACSClientBuilder::new()
                .managed_identity()
                .host(host_name.as_str())
```

- Send Email
```rust
    let email_request = SentEmailBuilder::new()
        .sender(sender.to_owned())
        .content(EmailContent {
            subject: Some("An exciting offer especially for you!".to_string()),
            plain_text: Some("This exciting offer was created especially for you, our most loyal customer.".to_string()),
            html: Some("<html><head><title>Exciting offer!</title></head><body><h1>This exciting offer was created especially for you, our most loyal customer.</h1></body></html>".to_string()),
        })
        .recipients(Recipients {
            to: Some(vec![EmailAddress {
                email: Some(recipient.to_owned()),
                display_name: Some(display_name.to_owned()),
            }]),
            cc: None,
            b_cc: None,
        })
        .user_engagement_tracking_disabled(false)
        .build()
        .expect("Failed to build SentEmail");

    debug!("Email request: {:#?}", email_request);

    let acs_client = acs_client_builder
        .build()
        .expect("Failed to build ACSClient");

    let resp_send_email = acs_client.send_email(&email_request).await;
```

- Get Email Status
```rust
    let resp_send_email = acs_client.send_email(&email_request).await;
    match resp_send_email {
        Ok(message_resp_id) => {
            info!("Email was sent with message id: {}", message_resp_id);
            loop {
                tokio::time::sleep(time::Duration::from_secs(5)).await;
                let resp_status = acs_client.get_email_status(&message_resp_id).await;
                if let Ok(status) = resp_status {
                    info!("{}\r\n", status.to_string());
                    if matches!(
                        status,
                        EmailSendStatusType::Unknown
                            | EmailSendStatusType::Canceled
                            | EmailSendStatusType::Failed
                            | EmailSendStatusType::Succeeded
                    ) {
                        break;
                    }
                } else {
                    error!("Error getting email status: {:?}", resp_status);
                    break;
                }
            }
        }
        Err(e) => error!("Error sending email: {:?}", e),
    }
```