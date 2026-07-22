include!("lib.rs");

#[cfg(not(test))]
const _: Option<WorkflowNodeStatus> = None;

#[cfg(not(test))]
const _: std::marker::PhantomData<Utc> = std::marker::PhantomData;
