import { useState } from "react";
import "./App.css";

function App() {
  const [text, setText] = useState("");

  function handleRecord() {
    setText((prev) => prev + "hello world");
  }

  function handleCopy() {
    navigator.clipboard.writeText(text);
  }

  return (
    <main className="container">
      <button className="record-btn" onClick={handleRecord}>
        Record
      </button>
      <div className="text-box-container">
        <div className="text-box" spellCheck={false}>
          {text}
        </div>
        {text && (
          <button className="copy-btn" onClick={handleCopy}>
            Copy
          </button>
        )}
      </div>
    </main>
  );
}

export default App;
