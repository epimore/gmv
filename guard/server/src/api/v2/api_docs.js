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

  const schemaRows = (schema = {}) => {
    const properties = schema.properties || {};
    const required = new Set(schema.required || []);
    return Object.entries(properties).map(([name, property]) => [
      name,
      property.type || (property.$ref ? "对象" : "任意 JSON"),
      required.has(name) ? "是" : "否",
      property.description || "—",
    ]);
  };

  const rawJson = (value) => {
    const details = element("details", "json-details");
    details.append(element("summary", "", "查看原始 JSON 定义"));
    details.append(element("pre", "", JSON.stringify(value, null, 2)));
    return details;
  };

  const operationCard = ({ method, path, summary, description, scope, parameters, requestSchema, responses, raw }) => {
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
    info.append(element("span", "chip ok", "响应格式：JSON"));
    body.append(info);

    if (parameters.length) {
      body.append(element("h3", "", "请求参数"));
      body.append(table(["字段", "位置", "类型", "必填", "中文说明"], parameters.map((parameter) => [
        parameter.name,
        parameter.in === "path" ? "路径" : parameter.in === "query" ? "查询" : "请求头",
        parameter.schema?.type || "string",
        parameter.required ? "是" : "否",
        parameter.description || "—",
      ])));
    }

    if (requestSchema) {
      body.append(element("h3", "", "JSON 请求字段"));
      const rows = schemaRows(requestSchema);
      body.append(rows.length ? table(["字段", "类型", "必填", "中文说明"], rows) : element("div", "empty", requestSchema.description || "JSON 请求正文"));
    }

    body.append(element("h3", "", "返回值"));
    const responseRows = Object.entries(responses || {}).map(([status, response]) => [
      status,
      response.description || "—",
      response.content?.["application/json"] ? "application/json" : "无响应正文",
    ]);
    body.append(table(["状态码", "中文说明", "返回格式"], responseRows));
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
          method,
          path,
          summary: operation.summary || "未命名接口",
          description: operation.description || "",
          scope: operation["x-gmv-required-scope"],
          parameters: operation.parameters || [],
          requestSchema,
          responses: operation.responses || {},
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

  const resolveRef = (spec, value) => {
    const ref = value?.$ref;
    if (!ref?.startsWith("#/")) return value || {};
    return ref.slice(2).split("/").reduce((current, key) => current?.[key], spec) || {};
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
        const rows = schemaRows(payload);
        body.append(rows.length ? table(["字段", "类型", "必填", "中文说明"], rows) : element("div", "empty", "该消息未声明字段"));
      });
      body.append(rawJson(channel));
      details.append(body);
      section.append(details);
    });
    content.append(section);
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
