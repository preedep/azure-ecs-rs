use azure_ecs_rs::adapters::gateways::acs_email::ACSClientBuilder;
use azure_ecs_rs::domain::entities::models::{
    EmailAddress, EmailAttachmentBuilder, EmailContent, EmailSendStatusType, Recipients,
    SentEmailBuilder,
};
use log::{debug, error, info};
use std::env;

/// Retrieves the value of an environment variable.
///
/// # Arguments
///
/// * `var_name` - The name of the environment variable.
///
/// # Returns
///
/// * `String` - The value of the environment variable.
fn get_env_var(var_name: &str) -> String {
    env::var(var_name).unwrap_or_else(|_| panic!("Environment variable {} is not set", var_name))
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();
    pretty_env_logger::init();

    let sender = get_env_var("SENDER");
    let recipient = get_env_var("REPLY_EMAIL");
    let display_name = get_env_var("REPLY_EMAIL_DISPLAY");

    let host_name = get_env_var("ASC_URL");
    let tenant_id = get_env_var("TENANT_ID");
    let client_id = get_env_var("CLIENT_ID");
    let client_secret = get_env_var("CLIENT_SECRET");

    // Create an ACS client by using the builder pattern
    let acs_client = ACSClientBuilder::new()
        .host(&host_name)
        .service_principal(&tenant_id, &client_id, &client_secret)
        .build()
        .expect("Failed to build ACS client");

    let mail_attach = EmailAttachmentBuilder::new()
        .file_to_base64("test_file.txt")
        .build()
        .expect("Failed to build EmailAttachment");

    let send_email = SentEmailBuilder::new()
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
        .attachments(vec![mail_attach])
        .user_engagement_tracking_disabled(false)
        .build()
        .expect("Failed to build SentEmail");

    debug!("Sending email... {:#?}", send_email);

    let res = acs_client
        .send_email_with_callback(&send_email, |msg_id, status, error| {
            info!(
                "Email with id: {} has status: {:?} and error: {:?}",
                msg_id, status, error
            );
        })
        .await;

    match res {
        Ok((id,rx)) => {
            if let _ = rx.await {
                info!("Email sent successfully with id: {}", id);
            }
        },
        Err(e) => error!("Failed to send email: {:?}", e),
    }
    Ok(())
}
