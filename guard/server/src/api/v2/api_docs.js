(() => {
  "use strict";

  const root = document.querySelector("[data-api-docs]");
  if (!root) return;

  const mode = root.dataset.mode;
  const specUrl = root.dataset.spec;
  const content = document.querySelector("#contract-content");
  const search = document.querySelector("#contract-search");
  const count = document.querySelector("#contract-count");

  const element = (name, className, text) => {
    const node = document.createElement(name);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  };

  const appendCell = (row, text, code = false) => {
    const cell = element("td");
    const value = code ? element("code", "", text) : document.createTextNode(text);
    cell.append(value);
    row.append(cell);
  };

  const table = (headers, rows) => {
    const wrap = element("div", "table-wrap");
    const node = document.createElement("table");
    const head = document.createElement("thead");
    const headerRow = document.createElement("tr");
    headers.forEach((header) => headerRow.append(element("th", "", header)));
    head.append(headerRow);
    node.append(head);
    const body = document.createElement("tbody");
    rows.forEach((values) => {
      const row = document.createElement("tr");
      values.forEach((value, index) => appendCell(row, value, index === 0));
      body.append(row);
    });
    node.append(body);
    wrap.append(node);
    return wrap;
  };

  const resolveRef = (spec, value) => {
    const ref = value?.$ref;
    if (!ref?.startsWith("#/")) return value || {};
    return ref.slice(2).split("/").reduce((current, key) => current?.[key], spec) || {};
  };

  const schemaType = (property = {}) => {
    const type = Array.isArray(property.type) ? property.type.join(" | ") : property.type;
    if (type === "array") {
      const itemType = Array.isArray(property.items?.type) ? property.items.type.join(" | ") : property.items?.type;
      return `array<${itemType || (property.items?.$ref ? "object" : "JSON")}>`;
    }
    return type || (property.$ref ? "object" : property.oneOf ? "oneOf" : "JSON");
  };

  const schemaConstraints = (property = {}) => [
    property.enum ? `可选：${property.enum.join("、")}` : "",
    property.const !== undefined ? `固定：${property.const}` : "",
    property.minimum !== undefined || property.maximum !== undefined ? `范围：${property.minimum ?? "-∞"}～${property.maximum ?? "+∞"}` : "",
    property.minLength !== undefined || property.maxLength !== undefined ? `长度：${property.minLength ?? 0}～${property.maxLength ?? "不限"}` : "",
    property.default !== undefined ? `默认：${property.default}` : "",
    property.pattern ? `格式：${property.pattern}` : "",
  ].filter(Boolean).join("；") || "—";

  const schemaRows = (schema = {}, spec = {}) => {
    const rows = [];
    const visit = (currentSchema, prefix = "", parentRequired = true, depth = 0) => {
      if (depth > 8) return;
      const resolvedSchema = resolveRef(spec, currentSchema);
      if (resolvedSchema.type === "array" || resolvedSchema.items) {
        visit(resolveRef(spec, resolvedSchema.items || {}), prefix ? `${prefix}[]` : "[]", parentRequired, depth + 1);
        return;
      }
      const properties = resolvedSchema.properties || {};
      const required = new Set(resolvedSchema.required || []);
      Object.entries(properties).forEach(([name, rawProperty]) => {
        const property = resolveRef(spec, rawProperty);
        const fieldName = prefix ? `${prefix}.${name}` : name;
        const isRequired = parentRequired && required.has(name);
        rows.push([
          fieldName,
          schemaType(property),
          isRequired ? "是" : "否",
          schemaConstraints(property),
          property.description || "—",
        ]);
        if (property.type === "array" || property.items) {
          visit(resolveRef(spec, property.items || {}), `${fieldName}[]`, isRequired, depth + 1);
        } else if (property.properties || property.$ref) {
          visit(property, fieldName, isRequired, depth + 1);
        }
      });
    };
    visit(schema);
    return rows;
  };

  const rawJson = (value) => {
    const details = element("details", "json-details");
    details.append(element("summary", "", "查看原始 JSON 定义"));
    details.append(element("pre", "", JSON.stringify(value, null, 2)));
    return details;
  };

  const appendExample = (body, title, value) => {
    if (value === undefined) return;
    body.append(element("h3", "", title));
    body.append(element("pre", "", JSON.stringify(value, null, 2)));
  };

  const appendEventPayloadContracts = (body, eventTypes, spec, addressFor) => {
    eventTypes.forEach((event) => {
      body.append(element("h3", "", `${event.event_type} · Payload 字段`));
      body.append(element("p", "description", `${addressFor(event)}；${event.description || "—"}`));
      const rows = schemaRows(event.payload_schema || {}, spec);
      body.append(rows.length
        ? table(["payload 字段", "类型", "必填", "取值约束", "中文说明"], rows)
        : element("div", "empty", "该事件尚未声明 Payload 字段"));
      appendExample(body, `${event.event_type} 完整消息示例`, event.envelope_example);
    });
  };

  const operationCard = ({ spec, method, path, summary, description, scope, parameters, requestSchema, requestExample, responses, eventTypes, callbackUrlSource, raw }) => {
    const details = element("details", "operation");
    details.dataset.search = `${method} ${path} ${summary} ${description}`.toLowerCase();
    const head = element("summary", "operation-head");
    head.append(element("span", `method ${method.toLowerCase()}`, method.toUpperCase()));
    head.append(element("code", "path", path));
    head.append(element("span", "summary-text", summary));
    head.append(element("span", "chevron", "›"));
    details.append(head);

    const body = element("div", "operation-body");
    body.append(element("p", "description", description || "—"));
    const info = element("div", "info-row");
    if (scope) info.append(element("span", "chip", `所需权限：${scope}`));
    if (callbackUrlSource) info.append(element("span", "chip", `回调地址：${callbackUrlSource}`));
    info.append(element("span", "chip ok", "响应格式：JSON"));
    body.append(info);

    if (eventTypes?.length) {
      body.append(element("h3", "", "可回调事件接口"));
      body.append(table(["事件类型", "方法", "回调路径", "Payload Profile", "用途", "说明"], eventTypes.map((event) => [
        event.event_type,
        event.method || "POST",
        `{callback_url}${event.http_path_suffix || ""}`,
        event.payload_profile || "event-envelope-v1",
        event.summary || "—",
        event.description || "—",
      ])));
      appendEventPayloadContracts(body, eventTypes, spec, (event) => `POST {callback_url}${event.http_path_suffix || ""}`);
    }

    if (parameters.length) {
      body.append(element("h3", "", "请求参数"));
      body.append(table(["字段", "位置", "类型", "必填", "取值约束", "中文说明"], parameters.map((parameter) => [
        parameter.name,
        parameter.in === "path" ? "路径" : parameter.in === "query" ? "查询" : "请求头",
        parameter.schema?.type || "string",
        parameter.required ? "是" : "否",
        schemaConstraints(parameter.schema || {}),
        parameter.description || "—",
      ])));
    }

    if (requestSchema) {
      body.append(element("h3", "", "JSON 请求字段"));
      const rows = schemaRows(requestSchema, spec);
      body.append(rows.length ? table(["字段", "类型", "必填", "取值约束", "中文说明"], rows) : element("div", "empty", requestSchema.description || "JSON 请求正文"));
    }
    appendExample(body, "完整请求示例", requestExample);

    body.append(element("h3", "", "返回值"));
    const responseRows = Object.entries(responses || {}).map(([status, response]) => [
      status,
      response.description || "—",
      response.content?.["application/json"] ? "application/json" : "无响应正文",
    ]);
    body.append(table(["状态码", "中文说明", "返回格式"], responseRows));
    const responseEntries = Object.entries(responses || {});
    const successEntry = responseEntries.find(([status]) => /^2\d\d$/.test(status));
    const errorEntry = responseEntries.find(([status]) => /^4\d\d$/.test(status));
    [["成功响应字段", successEntry], ["错误响应字段（各错误状态码同一结构）", errorEntry]].forEach(([title, entry]) => {
      if (!entry) return;
      const [status, response] = entry;
      const media = response.content?.["application/json"];
      if (!media?.schema) return;
      body.append(element("h3", "", `${title} · HTTP ${status}`));
      const rows = schemaRows(media.schema, spec);
      body.append(rows.length ? table(["字段", "类型", "必填", "取值约束", "中文说明"], rows) : element("div", "empty", media.schema.description || "无响应字段"));
      appendExample(body, `${status} 响应示例`, media.example);
    });
    body.append(rawJson(raw));
    details.append(body);
    return details;
  };

  const renderHttp = (spec) => {
    const groups = new Map();
    Object.entries(spec.paths || {}).forEach(([path, pathItem]) => {
      Object.entries(pathItem).forEach(([method, operation]) => {
        const tag = operation.tags?.[0] || "其他接口";
        if (!groups.has(tag)) groups.set(tag, []);
        const requestSchema = operation.requestBody?.content?.["application/json"]?.schema;
        groups.get(tag).push(operationCard({
          spec,
          method,
          path,
          summary: operation.summary || "未命名接口",
          description: operation.description || "",
          scope: operation["x-gmv-required-scope"],
          parameters: operation.parameters || [],
          requestSchema,
          requestExample: operation["x-gmv-request-example"],
          responses: operation.responses || {},
          raw: operation,
        }));
      });
    });
    Object.entries(spec.webhooks || {}).forEach(([name, pathItem]) => {
      Object.entries(pathItem).forEach(([method, operation]) => {
        const tag = "Guard 回调第三方";
        if (!groups.has(tag)) groups.set(tag, []);
        const requestMedia = operation.requestBody?.content?.["application/json"];
        groups.get(tag).push(operationCard({
          spec,
          method,
          path: operation["x-gmv-callback-url-source"] || name,
          summary: operation.summary || "Guard 事件回调",
          description: operation.description || "",
          parameters: operation.parameters || [],
          requestSchema: requestMedia?.schema,
          requestExample: requestMedia?.example,
          responses: operation.responses || {},
          eventTypes: operation["x-gmv-event-types"] || [],
          callbackUrlSource: operation["x-gmv-callback-url-source"],
          raw: operation,
        }));
      });
    });

    groups.forEach((cards, tag) => {
      const section = element("section", "group");
      const head = element("div", "group-head");
      head.append(element("h2", "", tag));
      head.append(element("span", "", `${cards.length} 个接口`));
      section.append(head);
      cards.forEach((card) => section.append(card));
      content.append(section);
    });
  };

  const renderMqtt = (spec) => {
    const section = element("section", "group");
    const head = element("div", "group-head");
    head.append(element("h2", "", "MQTT Channel 与消息"));
    const channelEntries = Object.entries(spec.channels || {});
    head.append(element("span", "", `${channelEntries.length} 个 Channel`));
    section.append(head);

    channelEntries.forEach(([name, channel]) => {
      const direction = name === "commands" ? "SUBSCRIBE" : "PUBLISH";
      const details = element("details", "operation");
      details.dataset.search = `${direction} ${channel.address || name} ${channel.description || ""}`.toLowerCase();
      const summary = element("summary", "operation-head");
      summary.append(element("span", `method ${direction.toLowerCase()}`, direction));
      summary.append(element("code", "path", channel.address || name));
      summary.append(element("span", "summary-text", channel.description || "MQTT 消息通道"));
      summary.append(element("span", "chevron", "›"));
      details.append(summary);

      const body = element("div", "operation-body");
      body.append(element("p", "description", direction === "SUBSCRIBE" ? "Guard 从该主题订阅第三方命令。" : "Guard 向该主题发布消息，第三方按业务标识幂等消费。"));
      const info = element("div", "info-row");
      info.append(element("span", "chip", `Guard 操作：${direction}`));
      info.append(element("span", "chip ok", "QoS：1"));
      info.append(element("span", "chip ok", "Payload：JSON"));
      body.append(info);

      const eventTypes = channel["x-gmv-event-types"] || [];
      if (eventTypes.length) {
        body.append(element("h3", "", "可发布事件列表"));
        body.append(table(["事件类型", "Topic", "Payload Profile", "用途", "说明"], eventTypes.map((event) => [
          event.event_type,
          `gmv/events/{integration_id}/${event.mqtt_topic_suffix}`,
          event.payload_profile || "event-envelope-v1",
          event.summary || "—",
          event.description || "—",
        ])));
        appendEventPayloadContracts(body, eventTypes, spec, (event) => `Topic gmv/events/{integration_id}/${event.mqtt_topic_suffix}`);
      }

      Object.entries(channel.parameters || {}).forEach(([parameterName, parameter]) => {
        const resolved = resolveRef(spec, parameter);
        body.append(element("h3", "", `Topic 参数：${parameterName}`));
        body.append(element("p", "description", resolved.description || "—"));
      });

      Object.entries(channel.messages || {}).forEach(([messageName, messageRef]) => {
        const message = resolveRef(spec, messageRef);
        const payload = resolveRef(spec, message.payload || {});
        body.append(element("h3", "", message.title || messageName));
        body.append(element("p", "description", message.summary || "JSON 消息体"));
        const rows = schemaRows(payload, spec);
        body.append(rows.length ? table(["字段", "类型", "必填", "取值约束", "中文说明"], rows) : element("div", "empty", "该消息未声明字段"));
        (payload.examples || []).forEach((example) => {
          appendExample(body, `${example.name || message.title || messageName} 完整消息示例`, example.payload ?? example);
        });
      });
      body.append(rawJson(channel));
      details.append(body);
      section.append(details);
    });
    content.append(section);

    const workflow = spec["x-gmv-mqtt-workflow"] || {};
    if (Array.isArray(workflow.steps)) {
      const workflowSection = element("section", "group");
      const workflowHead = element("div", "group-head");
      workflowHead.append(element("h2", "", "MQTT 调用闭环"));
      workflowHead.append(element("span", "", "发布前订阅结果，业务终态以 result 为准"));
      workflowSection.append(workflowHead);
      const workflowBody = element("div", "operation-body");
      const steps = document.createElement("ol");
      workflow.steps.forEach((step) => steps.append(element("li", "", step)));
      workflowBody.append(steps);
      workflowBody.append(element("p", "description", workflow.retry || ""));
      workflowBody.append(element("p", "description", workflow.security || ""));
      workflowSection.append(workflowBody);
      content.append(workflowSection);
    }

    const usageEntries = Object.entries(spec["x-gmv-action-usage"] || {});
    if (usageEntries.length) {
      const actionSection = element("section", "group");
      const actionHead = element("div", "group-head");
      actionHead.append(element("h2", "", "MQTT Action 请求与结果"));
      actionHead.append(element("span", "", `${usageEntries.length} 个 Action`));
      actionSection.append(actionHead);
      const commandMessage = spec.components?.messages?.IntegrationCommand;
      const commandPayload = resolveRef(spec, commandMessage?.payload || {});
      const examples = commandPayload.examples || [];
      const actionExamples = spec["x-gmv-action-examples"] || {};

      usageEntries.forEach(([action, usage]) => {
        const payloadSchema = spec.components?.schemas?.[usage.payload_schema] || {};
        const resultSchema = spec.components?.schemas?.[usage.result_schema] || {};
        const details = element("details", "operation");
        details.dataset.search = `${action} ${usage.summary || ""} ${usage.target || ""} ${(usage.http_equivalents || []).join(" ")} ${payloadSchema.description || ""}`.toLowerCase();
        const summary = element("summary", "operation-head");
        summary.append(element("span", "method publish", "ACTION"));
        summary.append(element("code", "path", action));
        summary.append(element("span", "summary-text", usage.summary || payloadSchema.description || "MQTT 命令"));
        summary.append(element("span", "chevron", "›"));
        details.append(summary);

        const body = element("div", "operation-body");
        body.append(element("p", "description", usage.summary || payloadSchema.description || "MQTT 命令"));
        const info = element("div", "info-row");
        info.append(element("span", "chip", `target：${usage.target || "见契约"}`));
        info.append(element("span", "chip", `所需权限：${usage.required_scope || "—"}`));
        info.append(element("span", "chip ok", "QoS：1 / retain=false"));
        body.append(info);
        if (Array.isArray(usage.http_equivalent_operations) && usage.http_equivalent_operations.length) {
          usage.http_equivalent_operations.forEach((operation) => {
            const equivalent = element("p", "description");
            equivalent.append(document.createTextNode("HTTP 等价接口："));
            equivalent.append(element("code", "", `${operation.method} ${operation.path}`));
            equivalent.append(document.createTextNode(` ${operation.summary || ""}`));
            body.append(equivalent);
          });
        } else if (Array.isArray(usage.http_equivalents) && usage.http_equivalents.length) {
          body.append(element("p", "description", `HTTP 等价接口：${usage.http_equivalents.join("；")}`));
        }
        body.append(element("h3", "", "payload 请求字段"));
        const payloadRows = schemaRows(payloadSchema, spec);
        body.append(payloadRows.length ? table(["字段", "类型", "必填", "取值约束", "中文说明"], payloadRows) : element("div", "empty", "payload 发送空对象 {}"));
        body.append(element("h3", "", "成功 result 字段"));
        const resultRows = schemaRows(resultSchema, spec);
        body.append(resultRows.length ? table(["字段", "类型", "必填", "取值约束", "中文说明"], resultRows) : element("div", "empty", "无额外结果字段"));
        if (usage.next) body.append(element("p", "description", `后续处理：${usage.next}`));
        const example = actionExamples[action] || {};
        const legacyExample = examples.find((item) => item.name === action);
        appendExample(body, "完整 MQTT 请求示例", example.request || legacyExample?.payload);
        appendExample(body, "成功响应示例", example.success);
        appendExample(body, "失败响应示例", example.failure);
        body.append(rawJson({ action, usage, payload: payloadSchema, result: resultSchema }));
        details.append(body);
        actionSection.append(details);
      });
      content.append(actionSection);
    }
  };

  const applyFilter = () => {
    const keyword = search.value.trim().toLowerCase();
    let visible = 0;
    document.querySelectorAll("details.operation").forEach((card) => {
      const matched = !keyword || card.dataset.search.includes(keyword);
      card.classList.toggle("hidden", !matched);
      if (matched) visible += 1;
    });
    document.querySelectorAll("section.group").forEach((group) => {
      group.classList.toggle("hidden", !group.querySelector("details.operation:not(.hidden)"));
    });
    count.textContent = `${visible} 项`;
  };

  fetch(specUrl, { credentials: "same-origin", headers: { Accept: "application/json" } })
    .then((response) => {
      if (!response.ok) throw new Error(`契约加载失败（HTTP ${response.status}）`);
      return response.json();
    })
    .then((spec) => {
      content.textContent = "";
      if (mode === "http") renderHttp(spec); else renderMqtt(spec);
      search.disabled = false;
      search.addEventListener("input", applyFilter);
      applyFilter();
    })
    .catch((error) => {
      content.textContent = "";
      content.append(element("div", "error", `${error.message}，请确认已使用管理员账号登录。`));
    });
})();
