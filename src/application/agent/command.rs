//! Decode untrusted wire fields once. Workflows receive domain values, not JSON.
use super::{Fault, WireResult};
use crate::{
    application::editing::TaskEdit,
    domain::{
        ids::{EntryId, TaskId},
        model::ActivityTemplate,
        pomodoro::Config,
        tasks::{Quadrant, Task, TaskInput, TaskStatus},
    },
};
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Deserializer, de::DeserializeOwned};
use serde_json::Value;

#[derive(Debug, Clone)]
pub(super) enum Command {
    Capabilities,
    Initialize,
    Context {
        limit: u32,
    },
    Query(Query),
    Receipt {
        request_id: String,
    },
    TaskAdd(TaskInput),
    TaskEdit {
        id: TaskId,
        patch: TaskPatch,
    },
    TaskStatus {
        id: TaskId,
        status: TaskStatus,
    },
    Start(ActivityTemplate),
    Add {
        activity: ActivityTemplate,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        zone: Option<Tz>,
    },
    StartTask(TaskId),
    Stop,
    Resume(EntryId),
    Edit {
        id: EntryId,
        patch: EntryPatch,
    },
    Delete(EntryId),
    PomoStart {
        activity: ActivityTemplate,
        config: Config,
    },
    PomoStartTask {
        id: TaskId,
        config: Config,
    },
    PomoPause,
    PomoResume,
    PomoNext,
    PomoStop,
}

impl Command {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Capabilities => "capabilities",
            Self::Initialize => "initialize",
            Self::Context { .. } => "context",
            Self::Query(_) => "query",
            Self::Receipt { .. } => "receipt",
            Self::TaskAdd(_) => "task.add",
            Self::TaskEdit { .. } => "task.edit",
            Self::TaskStatus { .. } => "task.status",
            Self::Start(_) => "tracking.start",
            Self::Add { .. } => "tracking.add",
            Self::StartTask(_) => "tracking.start_task",
            Self::Stop => "tracking.stop",
            Self::Resume(_) => "tracking.resume",
            Self::Edit { .. } => "tracking.edit",
            Self::Delete(_) => "tracking.delete",
            Self::PomoStart { .. } => "pomo.start",
            Self::PomoStartTask { .. } => "pomo.start_task",
            Self::PomoPause => "pomo.pause",
            Self::PomoResume => "pomo.resume",
            Self::PomoNext => "pomo.next",
            Self::PomoStop => "pomo.stop",
        }
    }
    pub(super) fn is_write(&self) -> bool {
        !matches!(
            self,
            Self::Capabilities | Self::Context { .. } | Self::Query(_) | Self::Receipt { .. }
        )
    }
    pub(super) fn entry_id(&self) -> Option<EntryId> {
        match self {
            Self::Resume(id) | Self::Delete(id) | Self::Edit { id, .. } => Some(*id),
            _ => None,
        }
    }
    pub(super) fn task_id(&self) -> Option<TaskId> {
        match self {
            Self::TaskEdit { id, .. }
            | Self::TaskStatus { id, .. }
            | Self::StartTask(id)
            | Self::PomoStartTask { id, .. } => Some(*id),
            _ => None,
        }
    }
    pub(super) fn decode(operation: &str, input: &Value) -> WireResult<Self> {
        Ok(match operation {
            "capabilities" => Self::Capabilities,
            "initialize" => Self::Initialize,
            "context" => Self::Context {
                limit: read::<Page>(input)?.limit,
            },
            "query" => Self::Query(read(input)?),
            "receipt" => Self::Receipt {
                request_id: read::<ReceiptInput>(input)?.request_id,
            },
            "task.add" => {
                let value: TaskAddInput = read(input)?;
                let activity = value
                    .activity
                    .map(ActivityInput::into_template)
                    .unwrap_or_else(|| ActivityTemplate {
                        activity: value.title.clone(),
                        category: None,
                        description: String::new(),
                        tags: Vec::new(),
                    });
                let task = TaskInput {
                    title: value.title,
                    activity,
                    quadrant: parse_quadrant(value.quadrant)?,
                    due_date: parse_date(value.due_date)?,
                };
                task.validate()?;
                Self::TaskAdd(task)
            }
            "task.edit" => {
                let value: TaskEditInput = read(input)?;
                let patch = TaskPatch {
                    title: value.title,
                    activity: value.activity,
                    description: value.description,
                    tags: value.tags.map(sorted_tags),
                    quadrant: value
                        .quadrant
                        .into_option()
                        .map(parse_quadrant)
                        .transpose()?,
                    due_date: value.due_date.into_option().map(parse_date).transpose()?,
                };
                if patch.is_empty() {
                    return Err(Fault::input("Supply at least one task field to edit"));
                }
                Self::TaskEdit {
                    id: TaskId::new(value.id)?,
                    patch,
                }
            }
            "task.status" => {
                let value: TaskStatusInput = read(input)?;
                Self::TaskStatus {
                    id: TaskId::new(value.id)?,
                    status: TaskStatus::parse(&value.status)?,
                }
            }
            "tracking.start" => Self::Start(read::<ActivityOnly>(input)?.activity.into_template()),
            "tracking.add" => {
                let value: AddInput = read(input)?;
                Self::Add {
                    activity: value.activity.into_template(),
                    start: parse_instant(&value.start)?,
                    end: parse_instant(&value.end)?,
                    zone: value
                        .zone
                        .map(|s| {
                            s.parse()
                                .map_err(|_| Fault::input("Unknown IANA time zone"))
                        })
                        .transpose()?,
                }
            }
            "tracking.start_task" => Self::StartTask(TaskId::new(read::<IdInput>(input)?.id)?),
            "tracking.stop" => Self::Stop,
            "tracking.resume" => Self::Resume(EntryId::new(read::<IdInput>(input)?.id)?),
            "tracking.delete" => Self::Delete(EntryId::new(read::<IdInput>(input)?.id)?),
            "tracking.edit" => {
                let value: EntryEditInput = read(input)?;
                let patch = EntryPatch {
                    activity: value.activity,
                    category: value.category.into_option(),
                    description: value.description,
                    tags: value.tags.map(sorted_tags),
                    start: value.start.map(|s| parse_instant(&s)).transpose()?,
                    end: value.end.map(|s| parse_instant(&s)).transpose()?,
                };
                if patch.is_empty() {
                    return Err(Fault::input("Supply at least one entry field to edit"));
                }
                Self::Edit {
                    id: EntryId::new(value.id)?,
                    patch,
                }
            }
            "pomo.start" => {
                let value: PomoStartInput = read(input)?;
                Self::PomoStart {
                    activity: value.activity.into_template(),
                    config: value.config.into_config(),
                }
            }
            "pomo.start_task" => {
                let value: PomoTaskInput = read(input)?;
                Self::PomoStartTask {
                    id: TaskId::new(value.id)?,
                    config: value.config.into_config(),
                }
            }
            "pomo.pause" => Self::PomoPause,
            "pomo.resume" => Self::PomoResume,
            "pomo.next" => Self::PomoNext,
            "pomo.stop" => Self::PomoStop,
            _ => return Err(Fault::input(format!("Unknown operation {operation:?}"))),
        })
    }
}

fn read<T: DeserializeOwned>(value: &Value) -> WireResult<T> {
    serde_json::from_value(value.clone())
        .map_err(|e| Fault::input(format!("Invalid operation input: {e}")))
}
fn sorted_tags(mut tags: Vec<String>) -> Vec<String> {
    tags.sort();
    tags.dedup();
    tags
}
fn parse_quadrant(value: Option<String>) -> WireResult<Option<Quadrant>> {
    Ok(value.as_deref().map(Quadrant::parse).transpose()?)
}
fn parse_date(value: Option<String>) -> WireResult<Option<NaiveDate>> {
    value
        .map(|s| {
            s.parse()
                .map_err(|_| Fault::input("due_date must be YYYY-MM-DD"))
        })
        .transpose()
}
fn parse_instant(value: &str) -> WireResult<DateTime<Utc>> {
    let timestamp = DateTime::parse_from_rfc3339(value)
        .map_err(|_| Fault::input("Expected RFC3339 timestamp with offset"))?;
    if timestamp.timestamp_subsec_nanos() != 0 {
        return Err(Fault::input("Timestamps must have whole-second precision"));
    }
    Ok(timestamp.to_utc())
}

/// Missing and explicit null have different patch semantics.
#[derive(Debug, Clone, Default)]
enum Field<T> {
    #[default]
    Missing,
    Present(T),
}
impl<T> Field<T> {
    fn into_option(self) -> Option<T> {
        match self {
            Self::Missing => None,
            Self::Present(v) => Some(v),
        }
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Field<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::Present)
    }
}

#[derive(Debug, Clone)]
pub(super) struct TaskPatch {
    title: Option<String>,
    activity: Option<ActivityPatch>,
    description: Option<String>,
    tags: Option<Vec<String>>,
    quadrant: Option<Option<Quadrant>>,
    due_date: Option<Option<NaiveDate>>,
}
impl TaskPatch {
    fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.activity.is_none()
            && self.description.is_none()
            && self.tags.is_none()
            && self.quadrant.is_none()
            && self.due_date.is_none()
    }
    pub(super) fn resolve(&self, old: &Task) -> TaskEdit {
        let activity = self.activity.as_ref().map(|v| ActivityTemplate {
            activity: v.activity.clone(),
            category: match &v.category {
                Field::Missing => old.category.clone(),
                Field::Present(v) => v.clone(),
            },
            description: old.description.clone(),
            tags: old.tags.clone(),
        });
        TaskEdit {
            title: self.title.clone(),
            activity,
            activity_name: None,
            category: None,
            notes: self
                .description
                .clone()
                .or_else(|| self.activity.as_ref().and_then(|a| a.description.clone())),
            tags: self.tags.clone().or_else(|| {
                self.activity
                    .as_ref()
                    .and_then(|a| a.tags.clone())
                    .map(sorted_tags)
            }),
            quadrant: self.quadrant,
            due: self.due_date,
        }
    }
}
#[derive(Debug, Clone)]
pub(super) struct EntryPatch {
    pub activity: Option<String>,
    pub category: Option<Option<String>>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
}
impl EntryPatch {
    fn is_empty(&self) -> bool {
        self.activity.is_none()
            && self.category.is_none()
            && self.description.is_none()
            && self.tags.is_none()
            && self.start.is_none()
            && self.end.is_none()
    }
}

pub(super) fn page_limit() -> u32 {
    20
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    #[serde(default = "page_limit")]
    limit: u32,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "entity", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Query {
    Tasks {
        #[serde(default)]
        after_id: i64,
        #[serde(default = "page_limit")]
        limit: u32,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        quadrant: NullableQuadrant,
        #[serde(default)]
        search: Option<String>,
    },
    Entries {
        #[serde(default)]
        after_id: i64,
        #[serde(default = "page_limit")]
        limit: u32,
        #[serde(default)]
        task_id: Option<i64>,
        #[serde(default)]
        quadrant: NullableQuadrant,
        #[serde(default)]
        search: Option<String>,
    },
    Operations {
        #[serde(default = "page_limit")]
        limit: u32,
    },
}
/// Public within the adapter so query code can preserve omitted vs inbox filtering.
#[derive(Debug, Clone, Default)]
pub(super) struct NullableQuadrant(Field<Option<String>>);
impl NullableQuadrant {
    pub(super) fn as_filter(&self) -> Option<Option<&str>> {
        match &self.0 {
            Field::Missing => None,
            Field::Present(v) => Some(v.as_deref()),
        }
    }
}
impl<'de> Deserialize<'de> for NullableQuadrant {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Field::deserialize(d).map(Self)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptInput {
    request_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdInput {
    id: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivityOnly {
    activity: ActivityInput,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivityInput {
    activity: String,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    tags: Vec<String>,
}
impl ActivityInput {
    fn into_template(self) -> ActivityTemplate {
        ActivityTemplate {
            activity: self.activity,
            category: self.category,
            description: self.description,
            tags: sorted_tags(self.tags),
        }
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivityPatch {
    activity: String,
    #[serde(default)]
    category: Field<Option<String>>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskAddInput {
    title: String,
    #[serde(default)]
    activity: Option<ActivityInput>,
    #[serde(default)]
    quadrant: Option<String>,
    #[serde(default)]
    due_date: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskEditInput {
    id: i64,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    activity: Option<ActivityPatch>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    quadrant: Field<Option<String>>,
    #[serde(default)]
    due_date: Field<Option<String>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskStatusInput {
    id: i64,
    status: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddInput {
    activity: ActivityInput,
    start: String,
    end: String,
    #[serde(default)]
    zone: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryEditInput {
    id: i64,
    #[serde(default)]
    activity: Option<String>,
    #[serde(default)]
    category: Field<Option<String>>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PomoStartInput {
    activity: ActivityInput,
    #[serde(default)]
    config: ConfigInput,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PomoTaskInput {
    id: i64,
    #[serde(default)]
    config: ConfigInput,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigInput {
    focus_seconds: Option<i64>,
    break_seconds: Option<i64>,
    long_break_seconds: Option<i64>,
    rounds: Option<u32>,
}
impl ConfigInput {
    fn into_config(self) -> Config {
        let defaults = Config::default();
        Config {
            focus_seconds: self.focus_seconds.unwrap_or(defaults.focus_seconds),
            break_seconds: self.break_seconds.unwrap_or(defaults.break_seconds),
            long_break_seconds: self
                .long_break_seconds
                .unwrap_or(defaults.long_break_seconds),
            rounds: self.rounds.unwrap_or(defaults.rounds),
        }
    }
}
