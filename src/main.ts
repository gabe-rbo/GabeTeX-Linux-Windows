import "./styles/main.css";

// Entry point. The UI agent builds the welcome/project windows here (plain DOM, no framework).
const app = document.getElementById("app");
if (app) app.textContent = "GabeTeX";
